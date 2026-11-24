//! Core scan, resolution, dry-run diff preview, rule-path audit, and `K1` batch staging
//! for Capability #12: Self-Healing Workspace (`4.1`) (`K1`–`K5`, `R1`, `R7`, `R8`, `R9`, `D1`).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, params};

use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::{build_policy_context, propose_batch};
use crate::db::files::{is_sensitive, path_is_within};
use crate::error::{AegisError, Result};
use crate::folderless::engine::validate_dest_root;
use crate::policy::{PolicyAction, PolicyVerdict, evaluate_policy};
use crate::versions::patterns::is_excluded_relative_path;
use crate::{new_id, now_ms};

use super::markdown::{
    ParsedLink, SpanReplacement, apply_span_replacements, compute_relative_markdown_target,
    encode_markdown_target, format_healed_link_preview, line_starts, parse_markdown_links,
};
use super::tags::{
    ScannedNoteTags, TagPatchTarget, analyze_tag_weeds, extract_tags_from_markdown,
};
use super::{
    BrokenLinkItem, HealBatchSummary, HealLinkKind, HealPatchPreviewFile, HealResolutionSource,
    HealResolutionStatus, HealScanOptions, HealScanReport, RulePathIssue, TagOccurrence,
    TagWeedCluster, TagWeedKind,
};

/// Agent persisted in `actions.agent` and `action_batches.origin` for Self-Healing Workspace (`K1`).
pub const HEALING_AGENT: &str = "self_healing";

const MAX_NOTE_BYTES: i64 = 2 * 1024 * 1024;
const MAX_HOPS: usize = 10;

#[derive(Clone)]
struct CatalogFile {
    id: String,
    path: String,
    name: String,
    ext: String,
    stem_lower: String,
    drive_label: String,
    case_sensitive: bool,
    sensitive: bool,
}

struct MoveEdge {
    old_norm: String,
    new_norm: String,
    new_raw: String,
    source: HealResolutionSource,
}

fn norm_key(raw: &str) -> String {
    raw.strip_prefix(r"\\?\")
        .unwrap_or(raw)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

fn lexically_collapse(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(comp);
                }
            }
            _ => out.push(comp),
        }
    }
    out
}

fn is_markdown_ext(ext: &str) -> bool {
    matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown")
}

fn load_dismissals(conn: &Connection) -> Result<HashSet<String>> {
    let mut st = conn.prepare("SELECT issue_key FROM heal_dismissals")?;
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    let mut set = HashSet::new();
    for r in rows {
        set.insert(r?);
    }
    Ok(set)
}

struct WorkspaceCatalog {
    live_files: Vec<CatalogFile>,
    by_norm: HashMap<String, CatalogFile>,
    by_stem_lower: HashMap<String, Vec<CatalogFile>>,
    by_filename_lower: HashMap<String, Vec<CatalogFile>>,
    quarantined_by_norm: HashMap<String, String>,
    move_edges: HashMap<String, Vec<MoveEdge>>,
    all_edges: Vec<MoveEdge>,
    move_log_entries: i64,
}

fn load_workspace_catalog(
    conn: &Connection,
    reveal_unlocked: bool,
) -> Result<WorkspaceCatalog> {
    let mut live_files = Vec::new();
    let mut by_norm: HashMap<String, CatalogFile> = HashMap::new();
    let mut by_stem_lower: HashMap<String, Vec<CatalogFile>> = HashMap::new();
    let mut by_filename_lower: HashMap<String, Vec<CatalogFile>> = HashMap::new();

    let mut stmt = conn.prepare(
        "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), COALESCE(v.label, v.mount_point),
                v.mount_point, COALESCE(v.case_sensitive, 0), COALESCE(f.sensitivity, 'normal')
         FROM files f
         JOIN volumes v ON v.id = f.volume_id
         WHERE f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND f.container_file_id IS NULL
           AND COALESCE(v.role, 'unassigned') != 'backup'
           AND v.status = 'online'",
    )?;

    for r in stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)? != 0,
            row.get::<_, String>(7)?,
        ))
    })? {
        let (id, path, name, ext, drive_label, mount_point, case_sensitive, sensitivity) = r?;
        if is_excluded_relative_path(&path, &mount_point) {
            continue;
        }
        let sensitive = is_sensitive(&sensitivity);
        if sensitive && !reveal_unlocked {
            continue;
        }
        let stem_lower = Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&name)
            .to_lowercase();
        let cf = CatalogFile {
            id,
            path: path.clone(),
            name: name.clone(),
            ext,
            stem_lower: stem_lower.clone(),
            drive_label,
            case_sensitive,
            sensitive,
        };
        by_norm.insert(norm_key(&path), cf.clone());
        by_stem_lower.entry(stem_lower).or_default().push(cf.clone());
        by_filename_lower
            .entry(name.to_lowercase())
            .or_default()
            .push(cf.clone());
        live_files.push(cf);
    }

    let mut quarantined_by_norm: HashMap<String, String> = HashMap::new();
    {
        let mut qst = conn.prepare(
            "SELECT id, original_path FROM quarantine_entries
             WHERE restored_at IS NULL AND purged_at IS NULL",
        )?;
        for r in qst.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (qid, orig) = r?;
            quarantined_by_norm.insert(norm_key(&orig), qid);
        }
    }

    let mut move_edges: HashMap<String, Vec<MoveEdge>> = HashMap::new();
    let mut all_edges: Vec<MoveEdge> = Vec::new();
    let mut push_edge = |old_p: String, new_p: String, src: HealResolutionSource| {
        let on = norm_key(&old_p);
        let nn = norm_key(&new_p);
        if on.is_empty() || nn.is_empty() || on == nn {
            return;
        }
        let edge = MoveEdge {
            old_norm: on.clone(),
            new_norm: nn,
            new_raw: new_p,
            source: src,
        };
        all_edges.push(MoveEdge {
            old_norm: edge.old_norm.clone(),
            new_norm: edge.new_norm.clone(),
            new_raw: edge.new_raw.clone(),
            source: edge.source,
        });
        move_edges.entry(on).or_default().push(edge);
    };

    let move_log_entries: i64 = conn
        .query_row("SELECT COUNT(*) FROM heal_move_log", [], |r| r.get(0))
        .unwrap_or(0);

    {
        let mut st = conn.prepare(
            "SELECT old_path, new_path FROM heal_move_log ORDER BY detected_at ASC, rowid ASC",
        )?;
        for r in st.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (old_p, new_p) = r?;
            push_edge(old_p, new_p, HealResolutionSource::HealMoveLog);
        }
    }
    {
        let mut st = conn.prepare(
            "SELECT old_path, new_path FROM file_history ORDER BY changed_at ASC, rowid ASC",
        )?;
        for r in st.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (old_p, new_p) = r?;
            push_edge(old_p, new_p, HealResolutionSource::FileHistory);
        }
    }
    {
        let mut st = conn.prepare(
            "SELECT from_path, to_path FROM actions
             WHERE status = 'executed' AND action_type IN ('move', 'rename') AND to_path IS NOT NULL
             ORDER BY executed_at ASC, created_at ASC",
        )?;
        for r in st.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))? {
            let (old_p, new_p) = r?;
            push_edge(old_p, new_p, HealResolutionSource::ActionLog);
        }
    }

    Ok(WorkspaceCatalog {
        live_files,
        by_norm,
        by_stem_lower,
        by_filename_lower,
        quarantined_by_norm,
        move_edges,
        all_edges,
        move_log_entries,
    })
}

enum MoveLookup<'a> {
    Live(&'a CatalogFile, HealResolutionSource),
    Quarantined(String),
}

fn lookup_via_move_graph<'a>(
    candidate_norm: &str,
    catalog: &'a WorkspaceCatalog,
) -> Option<MoveLookup<'a>> {
    if let Some(cf) = catalog.by_norm.get(candidate_norm)
        && Path::new(&cf.path).is_file()
    {
        return Some(MoveLookup::Live(cf, HealResolutionSource::HealMoveLog));
    }
    if let Some(qid) = catalog.quarantined_by_norm.get(candidate_norm) {
        return Some(MoveLookup::Quarantined(qid.clone()));
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize, HealResolutionSource)> = VecDeque::new();
    visited.insert(candidate_norm.to_string());

    let enqueue_edges_for = |cur_norm: &str,
                             next_hop: usize,
                             default_src: Option<HealResolutionSource>,
                             visited: &mut HashSet<String>,
                             queue: &mut VecDeque<(String, usize, HealResolutionSource)>| {
        if let Some(edges) = catalog.move_edges.get(cur_norm) {
            for edge in edges {
                if visited.insert(edge.new_norm.clone()) {
                    queue.push_back((
                        edge.new_norm.clone(),
                        next_hop,
                        default_src.unwrap_or(edge.source),
                    ));
                }
            }
        }
        for edge in &catalog.all_edges {
            let prefix = format!("{}/", edge.old_norm);
            if let Some(rest) = cur_norm.strip_prefix(&prefix) {
                let rebased = format!("{}/{rest}", edge.new_norm);
                if visited.insert(rebased.clone()) {
                    queue.push_back((rebased, next_hop, default_src.unwrap_or(edge.source)));
                }
            }
        }
    };

    enqueue_edges_for(candidate_norm, 1, None, &mut visited, &mut queue);

    while let Some((cur, hops, first_source)) = queue.pop_front() {
        if let Some(cf) = catalog.by_norm.get(&cur)
            && Path::new(&cf.path).is_file()
        {
            return Some(MoveLookup::Live(cf, first_source));
        }
        if let Some(qid) = catalog.quarantined_by_norm.get(&cur) {
            return Some(MoveLookup::Quarantined(qid.clone()));
        }
        if hops >= MAX_HOPS {
            continue;
        }
        enqueue_edges_for(&cur, hops + 1, Some(first_source), &mut visited, &mut queue);
    }
    None
}

fn candidate_target_paths(source_path: &Path, raw_target: &str, is_wiki: bool) -> Vec<(PathBuf, String)> {
    let os_rel = raw_target.replace('/', std::path::MAIN_SEPARATOR_STR);
    let target_p = Path::new(&os_rel);
    let base_abs = if target_p.is_absolute() {
        lexically_collapse(target_p)
    } else if let Some(parent) = source_path.parent() {
        lexically_collapse(&parent.join(target_p))
    } else {
        lexically_collapse(target_p)
    };
    let base_key = norm_key(&base_abs.to_string_lossy());
    let mut keys = vec![(base_abs.clone(), base_key)];
    if is_wiki && Path::new(raw_target).extension().is_none() {
        for ext in ["md", "markdown"] {
            let mut with_ext = base_abs.clone();
            let cur_name = with_ext
                .file_name()
                .map(|s| format!("{}.{ext}", s.to_string_lossy()))
                .unwrap_or_default();
            with_ext.set_file_name(cur_name);
            let k = norm_key(&with_ext.to_string_lossy());
            keys.push((with_ext, k));
        }
    }
    keys
}

fn candidate_target_norms(source_path: &Path, raw_target: &str, is_wiki: bool) -> Vec<String> {
    candidate_target_paths(source_path, raw_target, is_wiki)
        .into_iter()
        .map(|(_, k)| k)
        .collect()
}

fn target_is_currently_valid(
    source: &CatalogFile,
    link: &ParsedLink,
    catalog: &WorkspaceCatalog,
) -> bool {
    let is_wiki = matches!(link.kind, HealLinkKind::Wikilink | HealLinkKind::WikiEmbed);
    let src_p = Path::new(&source.path);

    for (raw_abs, cand) in candidate_target_paths(src_p, &link.target_path, is_wiki) {
        if let Some(cf) = catalog.by_norm.get(&cand)
            && Path::new(&cf.path).is_file()
        {
            return true;
        }
        // Fallback disk existence check when an asset lies outside scanned index.
        if raw_abs.is_file() {
            return true;
        }
    }

    // Obsidian short wikilink (`[[Unique Note]]`) resolves without a slash anywhere in catalog.
    if is_wiki && !link.target_path.contains('/') && !link.target_path.contains('\\') {
        let low = link.target_path.to_lowercase();
        let has_live_name = catalog
            .by_filename_lower
            .get(&low)
            .is_some_and(|list| list.iter().any(|c| Path::new(&c.path).is_file()));
        if has_live_name {
            return true;
        }
        let stem_no_ext = Path::new(&low)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&low);
        let has_live_stem = catalog
            .by_stem_lower
            .get(stem_no_ext)
            .is_some_and(|list| {
                list.iter()
                    .any(|c| is_markdown_ext(&c.ext) && Path::new(&c.path).is_file())
            });
        if has_live_stem {
            return true;
        }
    }

    false
}

fn format_wikilink_replacement(
    source_path: &Path,
    target: &CatalogFile,
    had_slash: bool,
    original_had_ext: bool,
    catalog: &WorkspaceCatalog,
) -> String {
    let stem_count = catalog
        .by_stem_lower
        .get(&target.stem_lower)
        .map(|v| v.len())
        .unwrap_or(0);
    if !had_slash && !original_had_ext && is_markdown_ext(&target.ext) && stem_count <= 1 {
        return Path::new(&target.name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| target.name.clone());
    }
    if let Some(rel) = compute_relative_markdown_target(source_path, Path::new(&target.path)) {
        if !original_had_ext
            && is_markdown_ext(&target.ext)
            && let Some(stripped) = rel.strip_suffix(".md").or_else(|| rel.strip_suffix(".markdown"))
        {
            return stripped.to_string();
        }
        return rel;
    }
    target.name.clone()
}

struct LinkPatchTarget {
    file_id: String,
    span: SpanReplacement,
}

struct WorkspaceAnalysis {
    report: HealScanReport,
    link_patches: HashMap<String, LinkPatchTarget>,
    tag_patches: HashMap<String, TagPatchTarget>,
    note_contents: HashMap<String, (String, String, String)>, // file_id -> (path, name, content)
}

fn resolve_broken_link(
    source: &CatalogFile,
    link: &ParsedLink,
    catalog: &WorkspaceCatalog,
    dismissed: &HashSet<String>,
) -> (BrokenLinkItem, Option<SpanReplacement>) {
    let issue_key = format!(
        "link:{}:{}:{}",
        source.id, link.target_span.0, link.target_span.1
    );
    let is_dismissed = dismissed.contains(&issue_key);
    let is_wiki = matches!(link.kind, HealLinkKind::Wikilink | HealLinkKind::WikiEmbed);
    let src_p = Path::new(&source.path);
    let candidates = candidate_target_norms(src_p, &link.target_path, is_wiki);

    // 1. Direct quarantine match on expected destination.
    for cand in &candidates {
        if let Some(qid) = catalog.quarantined_by_norm.get(cand) {
            return (
                BrokenLinkItem {
                    issue_key,
                    source_file_id: source.id.clone(),
                    source_path: source.path.clone(),
                    source_name: source.name.clone(),
                    drive_label: source.drive_label.clone(),
                    kind: link.kind,
                    line_number: link.line_number as i64,
                    raw_syntax: link.raw_syntax.clone(),
                    broken_target: link.target_path.clone(),
                    resolved_file_id: None,
                    resolved_path: None,
                    suggested_target: None,
                    replacement_syntax: None,
                    status: HealResolutionStatus::InQuarantine,
                    resolution_source: Some(HealResolutionSource::Quarantine),
                    confidence: 0.0,
                    quarantine_entry_id: Some(qid.clone()),
                    candidate_paths: Vec::new(),
                    sensitive: source.sensitive,
                    dismissed: is_dismissed,
                    reason: "Target was moved into Aegis 30-day Quarantine; restore it instead of rewriting links (K2).".into(),
                },
                None,
            );
        }
    }

    // 2. USN/scanner move log, file_history, and executed batch actions.
    for cand in &candidates {
        if let Some(res) = lookup_via_move_graph(cand, catalog) {
            match res {
                MoveLookup::Quarantined(qid) => {
                    return (
                        BrokenLinkItem {
                            issue_key,
                            source_file_id: source.id.clone(),
                            source_path: source.path.clone(),
                            source_name: source.name.clone(),
                            drive_label: source.drive_label.clone(),
                            kind: link.kind,
                            line_number: link.line_number as i64,
                            raw_syntax: link.raw_syntax.clone(),
                            broken_target: link.target_path.clone(),
                            resolved_file_id: None,
                            resolved_path: None,
                            suggested_target: None,
                            replacement_syntax: None,
                            status: HealResolutionStatus::InQuarantine,
                            resolution_source: Some(HealResolutionSource::Quarantine),
                            confidence: 0.0,
                            quarantine_entry_id: Some(qid),
                            candidate_paths: Vec::new(),
                            sensitive: source.sensitive,
                            dismissed: is_dismissed,
                            reason: "Moved file later entered Aegis 30-day Quarantine; restore it rather than rewriting links (K2).".into(),
                        },
                        None,
                    );
                }
                MoveLookup::Live(cf, src_kind) => {
                    let had_slash =
                        link.target_path.contains('/') || link.target_path.contains('\\');
                    let orig_had_ext = Path::new(&link.target_path).extension().is_some();
                    let new_target = if is_wiki {
                        format_wikilink_replacement(src_p, cf, had_slash, orig_had_ext, catalog)
                    } else {
                        let rel = compute_relative_markdown_target(src_p, Path::new(&cf.path))
                            .unwrap_or_else(|| cf.path.clone());
                        encode_markdown_target(&rel, link.use_angle_brackets)
                    };
                    let conf = match src_kind {
                        HealResolutionSource::HealMoveLog => 0.99,
                        HealResolutionSource::FileHistory => 0.97,
                        HealResolutionSource::ActionLog => 0.95,
                        _ => 0.90,
                    };
                    let source_desc = match src_kind {
                        HealResolutionSource::HealMoveLog => "USN / scanner move tracking",
                        HealResolutionSource::FileHistory => "file_history move log",
                        HealResolutionSource::ActionLog => "executed Aegis batch",
                        _ => "catalog match",
                    };
                    let rep_syntax = format_healed_link_preview(link, &new_target);
                    let span = SpanReplacement {
                        start: link.target_span.0,
                        end: link.target_span.1,
                        replacement: new_target.clone(),
                    };
                    return (
                        BrokenLinkItem {
                            issue_key,
                            source_file_id: source.id.clone(),
                            source_path: source.path.clone(),
                            source_name: source.name.clone(),
                            drive_label: source.drive_label.clone(),
                            kind: link.kind,
                            line_number: link.line_number as i64,
                            raw_syntax: link.raw_syntax.clone(),
                            broken_target: link.target_path.clone(),
                            resolved_file_id: Some(cf.id.clone()),
                            resolved_path: Some(cf.path.clone()),
                            suggested_target: Some(new_target),
                            replacement_syntax: Some(rep_syntax),
                            status: HealResolutionStatus::Resolved,
                            resolution_source: Some(src_kind),
                            confidence: conf,
                            quarantine_entry_id: None,
                            candidate_paths: vec![cf.path.clone()],
                            sensitive: source.sensitive,
                            dismissed: is_dismissed,
                            reason: format!("Resolved via {source_desc} to {}.", cf.path),
                        },
                        Some(span),
                    );
                }
            }
        }
    }

    // 3. Unique filename / stem fallback (refuses auto-patching if >= 2 candidates).
    let leaf_name = Path::new(&link.target_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&link.target_path)
        .to_lowercase();
    let leaf_stem = Path::new(&leaf_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&leaf_name)
        .to_string();

    let mut matched_live: Vec<&CatalogFile> = catalog
        .by_filename_lower
        .get(&leaf_name)
        .map(|v| v.iter().collect())
        .unwrap_or_default();
    if matched_live.is_empty()
        && is_wiki
        && Path::new(&link.target_path).extension().is_none()
        && let Some(stems) = catalog.by_stem_lower.get(&leaf_stem)
    {
        matched_live = stems.iter().filter(|c| is_markdown_ext(&c.ext)).collect();
    }
    matched_live.retain(|c| c.id != source.id && Path::new(&c.path).is_file());

    if matched_live.len() == 1 {
        let cf = matched_live[0];
        let had_slash = link.target_path.contains('/') || link.target_path.contains('\\');
        let orig_had_ext = Path::new(&link.target_path).extension().is_some();
        let new_target = if is_wiki {
            format_wikilink_replacement(src_p, cf, had_slash, orig_had_ext, catalog)
        } else {
            let rel = compute_relative_markdown_target(src_p, Path::new(&cf.path))
                .unwrap_or_else(|| cf.path.clone());
            encode_markdown_target(&rel, link.use_angle_brackets)
        };
        let rep_syntax = format_healed_link_preview(link, &new_target);
        let span = SpanReplacement {
            start: link.target_span.0,
            end: link.target_span.1,
            replacement: new_target.clone(),
        };
        return (
            BrokenLinkItem {
                issue_key,
                source_file_id: source.id.clone(),
                source_path: source.path.clone(),
                source_name: source.name.clone(),
                drive_label: source.drive_label.clone(),
                kind: link.kind,
                line_number: link.line_number as i64,
                raw_syntax: link.raw_syntax.clone(),
                broken_target: link.target_path.clone(),
                resolved_file_id: Some(cf.id.clone()),
                resolved_path: Some(cf.path.clone()),
                suggested_target: Some(new_target),
                replacement_syntax: Some(rep_syntax),
                status: HealResolutionStatus::Resolved,
                resolution_source: Some(HealResolutionSource::UniqueStem),
                confidence: 0.85,
                quarantine_entry_id: None,
                candidate_paths: vec![cf.path.clone()],
                sensitive: source.sensitive,
                dismissed: is_dismissed,
                reason: format!("Unique workspace filename match at {}.", cf.path),
            },
            Some(span),
        );
    }

    if matched_live.len() > 1 {
        let paths: Vec<String> = matched_live.iter().take(5).map(|c| c.path.clone()).collect();
        return (
            BrokenLinkItem {
                issue_key,
                source_file_id: source.id.clone(),
                source_path: source.path.clone(),
                source_name: source.name.clone(),
                drive_label: source.drive_label.clone(),
                kind: link.kind,
                line_number: link.line_number as i64,
                raw_syntax: link.raw_syntax.clone(),
                broken_target: link.target_path.clone(),
                resolved_file_id: None,
                resolved_path: None,
                suggested_target: None,
                replacement_syntax: None,
                status: HealResolutionStatus::Ambiguous,
                resolution_source: None,
                confidence: 0.0,
                quarantine_entry_id: None,
                candidate_paths: paths,
                sensitive: source.sensitive,
                dismissed: is_dismissed,
                reason: format!(
                    "Ambiguous target: {} live files share `{}` — choose target manually before rewriting.",
                    matched_live.len(),
                    leaf_name
                ),
            },
            None,
        );
    }

    (
        BrokenLinkItem {
            issue_key,
            source_file_id: source.id.clone(),
            source_path: source.path.clone(),
            source_name: source.name.clone(),
            drive_label: source.drive_label.clone(),
            kind: link.kind,
            line_number: link.line_number as i64,
            raw_syntax: link.raw_syntax.clone(),
            broken_target: link.target_path.clone(),
            resolved_file_id: None,
            resolved_path: None,
            suggested_target: None,
            replacement_syntax: None,
            status: HealResolutionStatus::Unresolved,
            resolution_source: None,
            confidence: 0.0,
            quarantine_entry_id: None,
            candidate_paths: Vec::new(),
            sensitive: source.sensitive,
            dismissed: is_dismissed,
            reason: "No matching file or recorded rename was found in catalog or move history.".into(),
        },
        None,
    )
}

fn resolve_dir_via_edges(dir_raw: &str, catalog: &WorkspaceCatalog) -> Option<String> {
    let dir_norm = norm_key(dir_raw);
    if let Some(edges) = catalog.move_edges.get(&dir_norm)
        && let Some(last) = edges.last()
        && Path::new(&last.new_raw).exists()
    {
        return Some(last.new_raw.clone());
    }
    for edge in &catalog.all_edges {
        let prefix = format!("{}/", edge.old_norm);
        if let Some(rest) = dir_norm.strip_prefix(&prefix) {
            let rebased = PathBuf::from(&edge.new_raw)
                .join(rest.replace('/', std::path::MAIN_SEPARATOR_STR));
            if rebased.exists() {
                return Some(rebased.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn audit_folderless_rules(
    conn: &Connection,
    catalog: &WorkspaceCatalog,
    dismissed: &HashSet<String>,
    include_dismissed: bool,
) -> Result<Vec<RulePathIssue>> {
    let mut issues = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT id, name, scope_root, dest_root FROM folderless_rules WHERE enabled = 1 ORDER BY name ASC",
    )?;
    for r in stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
        ))
    })? {
        let (rule_id, rule_name, scope_root, dest_root) = r?;

        if let Some(scope) = scope_root.as_deref().map(str::trim).filter(|s| !s.is_empty())
            && !Path::new(scope).exists()
        {
            let issue_key = format!("rule:{rule_id}:scope_root");
            let is_dismissed = dismissed.contains(&issue_key);
            if include_dismissed || !is_dismissed {
                let suggested = resolve_dir_via_edges(scope, catalog);
                let (status, reason) = match &suggested {
                    Some(new_p) => (
                        HealResolutionStatus::Resolved,
                        format!("Rule scope folder moved to {new_p}."),
                    ),
                    None => (
                        HealResolutionStatus::Unresolved,
                        format!("Rule scope folder `{scope}` no longer exists on disk."),
                    ),
                };
                issues.push(RulePathIssue {
                    issue_key,
                    rule_id: rule_id.clone(),
                    rule_name: rule_name.clone(),
                    field: "scope_root".into(),
                    current_path: scope.to_string(),
                    suggested_path: suggested,
                    status,
                    dismissed: is_dismissed,
                    reason,
                });
            }
        }

        let dest_trimmed = dest_root.trim();
        let dest_p = Path::new(dest_trimmed);
        if !dest_trimmed.is_empty() && dest_p.is_absolute() && !dest_p.exists() {
            let suggested = resolve_dir_via_edges(dest_trimmed, catalog);
            let parent_missing = dest_p.parent().is_none_or(|p| !p.exists());
            if suggested.is_some() || parent_missing {
                let issue_key = format!("rule:{rule_id}:dest_root");
                let is_dismissed = dismissed.contains(&issue_key);
                if include_dismissed || !is_dismissed {
                    let (status, reason) = match &suggested {
                        Some(new_p) => (
                            HealResolutionStatus::Resolved,
                            format!("Rule destination folder or ancestor moved to {new_p}."),
                        ),
                        None => (
                            HealResolutionStatus::Unresolved,
                            format!("Destination parent folder for `{dest_trimmed}` does not exist on disk."),
                        ),
                    };
                    issues.push(RulePathIssue {
                        issue_key,
                        rule_id: rule_id.clone(),
                        rule_name: rule_name.clone(),
                        field: "dest_root".into(),
                        current_path: dest_trimmed.to_string(),
                        suggested_path: suggested,
                        status,
                        dismissed: is_dismissed,
                        reason,
                    });
                }
            }
        }
    }
    Ok(issues)
}

fn analyze_workspace(
    conn: &Connection,
    opts: &HealScanOptions,
    reveal_sensitive: bool,
) -> Result<WorkspaceAnalysis> {
    if opts.include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before inspecting or healing sensitive notes (R7).".into(),
        ));
    }

    let started = Instant::now();
    let reveal_unlocked = opts.include_sensitive && reveal_sensitive;

    // Count sensitive markdown files first without loading or leaking text (R7).
    let mut excluded_sensitive_count = 0i64;
    if !reveal_unlocked {
        let mut sens_st = conn.prepare(
            "SELECT f.path, COALESCE(v.case_sensitive, 0)
             FROM files f
             JOIN volumes v ON v.id = f.volume_id
             WHERE f.deleted_at IS NULL
               AND f.state NOT IN ('quarantine', 'missing')
               AND COALESCE(v.role, 'unassigned') != 'backup'
               AND v.status = 'online'
               AND lower(COALESCE(f.ext, '')) IN ('md', 'markdown')
               AND COALESCE(f.sensitivity, 'normal') IN ('confidential', 'secret')",
        )?;
        for r in sens_st.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)))? {
            let (p, cs) = r?;
            if let Some(scope) = opts.scope_root.as_deref()
                && !path_is_within(&p, scope, cs)
            {
                continue;
            }
            excluded_sensitive_count += 1;
        }
    }

    let catalog = load_workspace_catalog(conn, reveal_unlocked)?;
    let dismissed = load_dismissals(conn)?;

    let mut notes_scanned = 0i64;
    let mut links_checked = 0i64;
    let mut broken_links = Vec::new();
    let mut link_patches: HashMap<String, LinkPatchTarget> = HashMap::new();
    let mut scanned_tags: Vec<ScannedNoteTags> = Vec::new();
    let mut note_contents: HashMap<String, (String, String, String)> = HashMap::new();

    let limit = opts.limit_notes.unwrap_or(usize::MAX);
    for note in &catalog.live_files {
        if notes_scanned as usize >= limit {
            break;
        }
        if !is_markdown_ext(&note.ext) {
            continue;
        }
        if let Some(scope) = opts.scope_root.as_deref()
            && !path_is_within(&note.path, scope, note.case_sensitive)
        {
            continue;
        }
        let disk_p = Path::new(&note.path);
        let Ok(meta) = std::fs::metadata(disk_p) else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() as i64 > MAX_NOTE_BYTES {
            continue;
        }
        let Ok(raw_bytes) = std::fs::read(disk_p) else {
            continue;
        };
        let Ok(text) = String::from_utf8(raw_bytes) else {
            continue;
        };

        notes_scanned += 1;
        let parsed_links = parse_markdown_links(&text);
        links_checked += parsed_links.len() as i64;
        for link in &parsed_links {
            if target_is_currently_valid(note, link, &catalog) {
                continue;
            }
            let (item, maybe_span) = resolve_broken_link(note, link, &catalog, &dismissed);
            if let Some(span) = maybe_span {
                link_patches.insert(
                    item.issue_key.clone(),
                    LinkPatchTarget {
                        file_id: note.id.clone(),
                        span,
                    },
                );
            }
            if opts.include_dismissed || !item.dismissed {
                broken_links.push(item);
            }
        }

        let extracted_tags = extract_tags_from_markdown(&text);
        if !extracted_tags.is_empty() {
            scanned_tags.push(ScannedNoteTags {
                file_id: note.id.clone(),
                file_path: note.path.clone(),
                file_name: note.name.clone(),
                sensitive: note.sensitive,
                tags: extracted_tags,
            });
        }

        note_contents.insert(
            note.id.clone(),
            (note.path.clone(), note.name.clone(), text),
        );
    }

    let (detected_weeds, tag_patches) = analyze_tag_weeds(&scanned_tags);
    let mut cluster_map: BTreeMap<(u8, String), Vec<_>> = BTreeMap::new();
    for weed in detected_weeds {
        let kind_ord = match weed.kind {
            TagWeedKind::HexColorFalsePositive => 0,
            TagWeedKind::CaseMismatch => 1,
            TagWeedKind::TypoOrVariant => 2,
        };
        cluster_map
            .entry((kind_ord, weed.canonical_tag.to_lowercase()))
            .or_default()
            .push(weed);
    }

    let mut tag_clusters = Vec::new();
    for ((kind_ord, key_lower), items) in cluster_map {
        let kind = match kind_ord {
            0 => TagWeedKind::HexColorFalsePositive,
            1 => TagWeedKind::CaseMismatch,
            _ => TagWeedKind::TypoOrVariant,
        };
        let cluster_prefix = match kind {
            TagWeedKind::HexColorFalsePositive => "hex",
            TagWeedKind::CaseMismatch => "case",
            TagWeedKind::TypoOrVariant => "typo",
        };
        let cluster_key = format!("tag_cluster:{cluster_prefix}:{key_lower}");
        let is_dismissed = dismissed.contains(&cluster_key);
        if !opts.include_dismissed && is_dismissed {
            continue;
        }

        let canonical_tag = items[0].canonical_tag.clone();
        let confidence = items.iter().map(|i| i.confidence).fold(0.0f64, f64::max);
        let reason = items[0].reason.clone();
        let mut variants = Vec::new();
        let mut occs = Vec::new();
        let mut patchable_count = 0i64;

        for w in items {
            if !variants.contains(&w.occurrence.raw_tag) {
                variants.push(w.occurrence.raw_tag.clone());
            }
            if w.occurrence.patchable {
                patchable_count += 1;
            }
            let occ_key = format!(
                "tag:{}:{}:{}",
                w.occurrence.file_id, w.occurrence.span.0, w.occurrence.span.1
            );
            occs.push(TagOccurrence {
                issue_key: occ_key,
                file_id: w.occurrence.file_id,
                file_path: w.occurrence.file_path,
                file_name: w.occurrence.file_name,
                line_number: w.occurrence.line_number as i64,
                in_frontmatter: w.occurrence.in_frontmatter,
                raw_tag: w.occurrence.raw_tag,
                canonical_tag: w.occurrence.canonical_tag,
                patchable: w.occurrence.patchable,
                sensitive: w.occurrence.sensitive,
                snippet: w.occurrence.snippet,
            });
        }
        let total_occurrences = occs.len() as i64;
        tag_clusters.push(TagWeedCluster {
            issue_key: cluster_key,
            kind,
            canonical_tag,
            variants,
            total_occurrences,
            patchable_count,
            confidence,
            dismissed: is_dismissed,
            reason,
            occurrences: occs,
        });
    }

    let rule_issues = audit_folderless_rules(
        conn,
        &catalog,
        &dismissed,
        opts.include_dismissed,
    )?;

    let active_broken: Vec<&BrokenLinkItem> =
        broken_links.iter().filter(|l| !l.dismissed).collect();
    let broken_links_count = active_broken.len() as i64;
    let healable_links_count = active_broken
        .iter()
        .filter(|l| l.status == HealResolutionStatus::Resolved)
        .count() as i64;
    let active_tag_clusters: Vec<&TagWeedCluster> =
        tag_clusters.iter().filter(|c| !c.dismissed).collect();
    let tag_weeds_count = active_tag_clusters
        .iter()
        .map(|c| c.total_occurrences)
        .sum::<i64>();
    let rule_path_issues_count = rule_issues.iter().filter(|r| !r.dismissed).count() as i64;

    let relational_integrity_score = if links_checked == 0 {
        100.0
    } else {
        let ratio = (broken_links_count as f64) / (links_checked as f64);
        ((1.0 - ratio) * 100.0).clamp(0.0, 100.0)
    };
    let taxonomic_hygiene_score =
        (100.0 - (tag_weeds_count as f64) * 5.0).clamp(15.0, 100.0);

    Ok(WorkspaceAnalysis {
        report: HealScanReport {
            notes_scanned,
            links_checked,
            broken_links_count,
            healable_links_count,
            tag_weeds_count,
            rule_path_issues_count,
            excluded_sensitive_count,
            relational_integrity_score: (relational_integrity_score * 10.0).round() / 10.0,
            taxonomic_hygiene_score: (taxonomic_hygiene_score * 10.0).round() / 10.0,
            move_log_entries: catalog.move_log_entries,
            broken_links,
            tag_clusters,
            rule_issues,
            duration_ms: started.elapsed().as_millis() as i64,
        },
        link_patches,
        tag_patches,
        note_contents,
    })
}

/// Scans all indexed non-backup Markdown notes & Folderless rules without modifying disk.
pub fn scan_workspace_healing(
    conn: &Connection,
    opts: &HealScanOptions,
    reveal_sensitive: bool,
) -> Result<HealScanReport> {
    Ok(analyze_workspace(conn, opts, reveal_sensitive)?.report)
}

struct CompiledFilePatch {
    file_id: String,
    file_path: String,
    file_name: String,
    sensitive: bool,
    original_content: String,
    patched_content: String,
    links_healed: i64,
    tags_pruned: i64,
    min_confidence: f64,
    diff_preview: String,
    reasons: Vec<String>,
}

fn build_line_diff(original: &str, patched: &str) -> String {
    let orig_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = patched.lines().collect();
    let starts = line_starts(original);
    let _ = starts;
    let mut out = String::new();
    let max = orig_lines.len().max(new_lines.len());
    for idx in 0..max {
        match (orig_lines.get(idx), new_lines.get(idx)) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                out.push_str(&format!("L{}: - {a}\nL{}: + {b}\n", idx + 1, idx + 1));
            }
            (Some(a), None) => {
                out.push_str(&format!("L{}: - {a}\n", idx + 1));
            }
            (None, Some(b)) => {
                out.push_str(&format!("L{}: + {b}\n", idx + 1));
            }
            (None, None) => {}
        }
    }
    if out.is_empty() {
        out.push_str("(no line delta)");
    }
    out
}

fn compile_selected_patches(
    conn: &Connection,
    link_issue_keys: &[String],
    tag_issue_keys: &[String],
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<Vec<CompiledFilePatch>> {
    let opts = HealScanOptions {
        scope_root: None,
        include_sensitive,
        include_dismissed: true,
        limit_notes: None,
    };
    let analysis = analyze_workspace(conn, &opts, reveal_sensitive)?;

    let link_conf: HashMap<String, (f64, String)> = analysis
        .report
        .broken_links
        .into_iter()
        .map(|l| (l.issue_key, (l.confidence, l.reason)))
        .collect();
    let mut tag_conf: HashMap<String, (f64, String)> = HashMap::new();
    let mut cluster_to_occs: HashMap<String, Vec<String>> = HashMap::new();
    for cl in &analysis.report.tag_clusters {
        let mut keys = Vec::new();
        for occ in &cl.occurrences {
            tag_conf.insert(occ.issue_key.clone(), (cl.confidence, cl.reason.clone()));
            keys.push(occ.issue_key.clone());
        }
        cluster_to_occs.insert(cl.issue_key.clone(), keys);
    }

    struct FileSpanAccumulator {
        spans: Vec<SpanReplacement>,
        links_healed: i64,
        tags_pruned: i64,
        min_conf: f64,
        reasons: Vec<String>,
    }

    impl FileSpanAccumulator {
        fn new() -> Self {
            Self {
                spans: Vec::new(),
                links_healed: 0,
                tags_pruned: 0,
                min_conf: 1.0,
                reasons: Vec::new(),
            }
        }
    }

    let mut per_file_spans: BTreeMap<String, FileSpanAccumulator> = BTreeMap::new();
    let mut seen_occ: HashSet<String> = HashSet::new();

    for key in link_issue_keys {
        if !seen_occ.insert(key.clone()) {
            continue;
        }
        if let Some(lp) = analysis.link_patches.get(key) {
            let (conf, reason) = link_conf
                .get(key)
                .cloned()
                .unwrap_or((0.90, "Broken link target updated".into()));
            let entry = per_file_spans
                .entry(lp.file_id.clone())
                .or_insert_with(FileSpanAccumulator::new);
            entry.spans.push(lp.span.clone());
            entry.links_healed += 1;
            entry.min_conf = entry.min_conf.min(conf);
            entry.reasons.push(reason);
        }
    }

    let mut expanded_tag_keys: Vec<String> = Vec::new();
    for key in tag_issue_keys {
        if let Some(occ_keys) = cluster_to_occs.get(key) {
            expanded_tag_keys.extend(occ_keys.iter().cloned());
        } else {
            expanded_tag_keys.push(key.clone());
        }
    }

    for key in &expanded_tag_keys {
        if !seen_occ.insert(key.clone()) {
            continue;
        }
        if let Some(tp) = analysis.tag_patches.get(key) {
            let (conf, reason) = tag_conf
                .get(key)
                .cloned()
                .unwrap_or((0.90, "Tag consolidated".into()));
            let entry = per_file_spans
                .entry(tp.file_id.clone())
                .or_insert_with(FileSpanAccumulator::new);
            entry.spans.push(tp.span.clone());
            entry.tags_pruned += 1;
            entry.min_conf = entry.min_conf.min(conf);
            entry.reasons.push(reason);
        }
    }

    let mut compiled = Vec::new();
    for (file_id, mut acc) in per_file_spans {
        let Some((file_path, file_name, original_content)) = analysis.note_contents.get(&file_id)
        else {
            continue;
        };
        // Deduplicate/remove any overlapping spans defensively.
        acc.spans
            .sort_by(|a, b| b.start.cmp(&a.start).then_with(|| b.end.cmp(&a.end)));
        let mut clean_spans = Vec::with_capacity(acc.spans.len());
        let mut prev_start = usize::MAX;
        for sp in acc.spans {
            if sp.end <= prev_start && sp.start <= sp.end {
                prev_start = sp.start;
                clean_spans.push(sp);
            }
        }
        if clean_spans.is_empty() {
            continue;
        }
        let patched_content = apply_span_replacements(original_content, clean_spans);
        if &patched_content == original_content {
            continue;
        }
        let sensitive: bool = conn
            .query_row(
                "SELECT COALESCE(sensitivity, 'normal') FROM files WHERE id = ?1",
                params![file_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .is_some_and(|s| is_sensitive(&s));
        let diff_preview = build_line_diff(original_content, &patched_content);
        compiled.push(CompiledFilePatch {
            file_id,
            file_path: file_path.clone(),
            file_name: file_name.clone(),
            sensitive,
            original_content: original_content.clone(),
            patched_content,
            links_healed: acc.links_healed,
            tags_pruned: acc.tags_pruned,
            min_confidence: acc.min_conf,
            diff_preview,
            reasons: acc.reasons,
        });
    }

    Ok(compiled)
}

/// Generates a non-destructive unified diff preview per file before staging `ActionType::PatchText` (`K1`).
pub fn preview_heal_patches(
    store: &SqliteStore,
    link_issue_keys: &[String],
    tag_issue_keys: &[String],
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<Vec<HealPatchPreviewFile>> {
    let ctx = build_policy_context(store)?;
    let compiled = compile_selected_patches(
        store.conn(),
        link_issue_keys,
        tag_issue_keys,
        include_sensitive,
        reveal_sensitive,
    )?;

    let mut out = Vec::with_capacity(compiled.len());
    for patch in compiled {
        let verdict = evaluate_policy(
            &PolicyAction {
                action_type: ActionType::PatchText,
                target_kind: TargetKind::File,
                agent: Some(HEALING_AGENT),
                from_path: Path::new(&patch.file_path),
                to_path: None,
                confidence: patch.min_confidence,
                atomic_unit_root: None,
                unit_allow_split_override: false,
                file_unit_split_override: false,
                is_registered_cache_dir: false,
            },
            &ctx,
        );
        let blocked_reason = match verdict {
            PolicyVerdict::Block(code) => Some(format!("policy_blocked:{code}")),
            _ => None,
        };
        out.push(HealPatchPreviewFile {
            file_id: patch.file_id,
            file_path: patch.file_path,
            file_name: patch.file_name,
            links_healed: patch.links_healed,
            tags_pruned: patch.tags_pruned,
            diff_preview: patch.diff_preview,
            blocked_reason,
            sensitive: patch.sensitive,
        });
    }
    Ok(out)
}

/// Stages an approvable `ActionType::PatchText` batch into the Simulator + `heal_patch_payloads` (`K1`).
pub fn propose_heal_batch(
    store: &mut SqliteStore,
    link_issue_keys: &[String],
    tag_issue_keys: &[String],
    include_sensitive: bool,
    reveal_sensitive: bool,
) -> Result<String> {
    if link_issue_keys.is_empty() && tag_issue_keys.is_empty() {
        return Err(AegisError::InvalidInput(
            "Select at least one healable link or tag weed to stage.".into(),
        ));
    }

    let ctx = build_policy_context(store)?;
    let compiled = compile_selected_patches(
        store.conn(),
        link_issue_keys,
        tag_issue_keys,
        include_sensitive,
        reveal_sensitive,
    )?;
    if compiled.is_empty() {
        return Err(AegisError::InvalidInput(
            "None of the selected issues can be auto-patched cleanly.".into(),
        ));
    }

    let mut actions = Vec::new();
    let mut payloads: Vec<(String, String, String, Vec<u8>, String, String)> = Vec::new();

    for patch in compiled {
        let open_exists: bool = store.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM actions WHERE file_id = ?1 AND status IN ('proposed', 'approved'))",
            params![patch.file_id],
            |r| r.get(0),
        )?;
        if open_exists {
            continue;
        }

        let atomic_unit_id: Option<String> = store
            .conn()
            .query_row(
                "SELECT atomic_unit_id FROM files WHERE id = ?1",
                params![patch.file_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let atomic_unit_root: Option<PathBuf> = match atomic_unit_id.as_deref() {
            Some(au_id) => store
                .conn()
                .query_row(
                    "SELECT root_path FROM atomic_units WHERE id = ?1",
                    params![au_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(PathBuf::from),
            None => None,
        };

        let verdict = evaluate_policy(
            &PolicyAction {
                action_type: ActionType::PatchText,
                target_kind: TargetKind::File,
                agent: Some(HEALING_AGENT),
                from_path: Path::new(&patch.file_path),
                to_path: None,
                confidence: patch.min_confidence,
                atomic_unit_root: atomic_unit_root.as_deref(),
                unit_allow_split_override: false,
                file_unit_split_override: false,
                is_registered_cache_dir: false,
            },
            &ctx,
        );
        if matches!(verdict, PolicyVerdict::Block(_)) {
            continue;
        }

        let orig_blake3 = blake3::hash(patch.original_content.as_bytes())
            .as_bytes()
            .to_vec();
        let patch_kind = match (patch.links_healed > 0, patch.tags_pruned > 0) {
            (true, true) => "mixed",
            (true, false) => "links",
            _ => "tags",
        }
        .to_string();
        let summary_json = serde_json::json!({
            "links_healed": patch.links_healed,
            "tags_pruned": patch.tags_pruned,
            "reasons": patch.reasons,
        })
        .to_string();

        let reason = format!(
            "Self-heal {}: {} link(s), {} tag(s)",
            patch.file_name, patch.links_healed, patch.tags_pruned
        );
        actions.push(ProposedActionInput {
            agent: Some(HEALING_AGENT.into()),
            action_type: ActionType::PatchText,
            target_kind: TargetKind::File,
            file_id: Some(patch.file_id.clone()),
            atomic_unit_id,
            from_path: patch.file_path,
            to_path: None,
            reason,
            confidence: patch.min_confidence,
            evidence_json: summary_json.clone(),
        });
        payloads.push((
            patch.file_id,
            patch.original_content,
            patch.patched_content,
            orig_blake3,
            patch_kind,
            summary_json,
        ));
    }

    if actions.is_empty() {
        return Err(AegisError::InvalidInput(
            "Selected files either already have pending actions in the Simulator or are outside configured roots.".into(),
        ));
    }

    let batch_id = propose_batch(
        store,
        &RefilePlan {
            origin: HEALING_AGENT.into(),
            actions,
        },
    )?;

    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        for (fid, orig, patched, orig_hash, pkind, sjson) in payloads {
            let action_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM actions WHERE batch_id = ?1 AND file_id = ?2 AND action_type = 'patch_text'",
                    params![batch_id, fid],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(aid) = action_id {
                tx.execute(
                    "INSERT INTO heal_patch_payloads
                        (action_id, original_content, patched_content, original_blake3, patch_kind, summary_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![aid, orig, patched, orig_hash.as_slice(), pkind, sjson, now],
                )?;
            }
        }
        Ok(())
    })?;

    Ok(batch_id)
}

pub fn dismiss_heal_issue(
    conn: &Connection,
    issue_key: &str,
    issue_kind: &str,
    note: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO heal_dismissals (id, issue_key, issue_kind, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(issue_key) DO UPDATE SET
            issue_kind = excluded.issue_kind,
            note = excluded.note,
            created_at = excluded.created_at",
        params![new_id(), issue_key, issue_kind, note, now_ms()],
    )?;
    Ok(())
}

pub fn undismiss_heal_issue(conn: &Connection, issue_key: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM heal_dismissals WHERE issue_key = ?1",
        params![issue_key],
    )?;
    Ok(())
}

/// Repairs a stale `folderless_rules` `scope_root` or `dest_root` entry inside `aegis.db`.
pub fn heal_folderless_rule_path(
    store: &mut SqliteStore,
    rule_id: &str,
    field: &str,
    new_path: &str,
) -> Result<()> {
    let trimmed = new_path.trim();
    validate_dest_root(trimmed)?;

    let mut st = store.conn().prepare(
        "SELECT mount_point, COALESCE(case_sensitive, 0) FROM volumes WHERE role = 'backup'",
    )?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))? {
        let (mount, cs) = row?;
        if path_is_within(trimmed, &mount, cs) {
            return Err(AegisError::PermissionDenied(
                "Healed rule path cannot target a backup volume (K4).".into(),
            ));
        }
    }
    drop(st);

    let now = now_ms();
    let changed = match field {
        "scope_root" => store.conn().execute(
            "UPDATE folderless_rules SET scope_root = ?1, updated_at = ?2 WHERE id = ?3",
            params![trimmed, now, rule_id],
        )?,
        "dest_root" => store.conn().execute(
            "UPDATE folderless_rules SET dest_root = ?1, updated_at = ?2 WHERE id = ?3",
            params![trimmed, now, rule_id],
        )?,
        _ => {
            return Err(AegisError::InvalidInput(format!(
                "Unsupported rule path field `{field}`."
            )));
        }
    };
    if changed == 0 {
        return Err(AegisError::NotFound(format!("folderless rule {rule_id}")));
    }
    Ok(())
}

pub fn list_heal_batches(conn: &Connection, limit: usize) -> Result<Vec<HealBatchSummary>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.status, b.created_at,
                COUNT(a.id) AS total_actions,
                SUM(CASE WHEN a.status = 'executed' THEN 1 ELSE 0 END) AS executed_cnt,
                SUM(CASE WHEN a.status = 'rolled_back' THEN 1 ELSE 0 END) AS rolled_back_cnt
         FROM action_batches b
         LEFT JOIN actions a ON a.batch_id = b.id
         WHERE b.origin = ?1
         GROUP BY b.id
         ORDER BY b.created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![HEALING_AGENT, limit as i64], |r| {
        let batch_id: String = r.get(0)?;
        let status: String = r.get(1)?;
        let created_at: i64 = r.get(2)?;
        let files_count: i64 = r.get(3)?;
        let executed_count: i64 = r.get(4)?;
        let rolled_back_count: i64 = r.get(5)?;
        Ok(HealBatchSummary {
            summary_label: format!("Workspace heal ({files_count} note(s))"),
            batch_id,
            status,
            files_count,
            executed_count,
            rolled_back_count,
            can_undo: executed_count > 0,
            created_at,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
