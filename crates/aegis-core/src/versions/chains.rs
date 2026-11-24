//! Multi-signal version chain assembly (`FilenamePattern`, `VersionsFolder`, `FileHistory`,
//! `SemanticSimilarity`), exact `blake3` duplicate detection, and pin/dismiss persistence.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, params};

use crate::db::files::{is_sensitive, path_is_within};
use crate::db::vec::{cosine_similarity, load_file_vectors};
use crate::error::{AegisError, Result};
use crate::now_ms;

use super::patterns::{
    ParsedVersionName, is_excluded_relative_path, is_generic_boilerplate_stem, parse_version_name,
};
use super::{VersionChain, VersionLinkReason, VersionNode, VersionScanOptions, VersionScanReport};

const DEFAULT_MIN_SIMILARITY: f64 = 0.92;
const MAX_SEMANTIC_BUCKET_FILES: usize = 200;

struct DbFileCandidate {
    file_id: String,
    volume_id: String,
    mount_point: String,
    drive_label: String,
    drive_online: bool,
    path: String,
    path_lower: String,
    filename: String,
    ext: String,
    size_bytes: i64,
    mtime: i64,
    blake3: Option<Vec<u8>>,
    sensitivity: String,
    parsed: ParsedVersionName,
    collection_id: Option<String>,
    collection_name: Option<String>,
    collection_root: Option<String>,
}

struct ActiveCollectionLoc {
    id: String,
    name: String,
    volume_id: String,
    root_path: String,
}

struct ChainPref {
    pinned_latest_file_id: Option<String>,
    dismissed: bool,
}

struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        while self.parent[x] != root {
            let next = self.parent[x];
            self.parent[x] = root;
            x = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[rb] = ra;
        }
    }
}

fn norm_lower(p: &str) -> String {
    p.strip_prefix(r"\\?\")
        .unwrap_or(p)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn join_dir(base: &str, leaf: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    format!("{}{sep}{leaf}", base.trim_end_matches(['\\', '/']))
}

fn semantic_ext_family(ext_group: &str) -> &str {
    match ext_group {
        "txt" | "md" => "text_doc",
        other => other,
    }
}

fn stable_chain_key(
    volume_id: &str,
    anchor_dir: &str,
    canonical_stem_lower: &str,
    ext_group: &str,
) -> String {
    let mut h = blake3::Hasher::new();
    h.update(volume_id.as_bytes());
    h.update(b"\0");
    h.update(norm_lower(anchor_dir).as_bytes());
    h.update(b"\0");
    h.update(canonical_stem_lower.as_bytes());
    h.update(b"\0");
    h.update(ext_group.as_bytes());
    h.finalize().to_hex()[..24].to_string()
}

/// Scans catalogued files (`K2`/`K4` compliant, `R7` safe) into ordered semantic version chains.
pub fn scan_version_chains(
    conn: &Connection,
    options: &VersionScanOptions,
    now_ms: i64,
    reveal_sensitive: bool,
) -> Result<VersionScanReport> {
    let collections = load_collection_locations(conn)?;
    let member_collections = load_collection_members(conn)?;
    let vol_mounts = load_volume_mounts(conn)?;
    let prefs = load_chain_prefs(conn)?;
    let mut files = load_candidate_files(
        conn,
        options.volume_id.as_deref(),
        &collections,
        &member_collections,
    )?;

    if files.is_empty() {
        return Ok(VersionScanReport {
            scanned_at: now_ms,
            total_chains: 0,
            total_versions: 0,
            chains_with_dupes: 0,
            reclaimable_duplicate_bytes: 0,
            semantic_chains_count: 0,
            versions_folder_chains_count: 0,
            dismissed_count: 0,
            chains: Vec::new(),
        });
    }

    let n = files.len();
    let mut dsu = DisjointSet::new(n);
    let mut node_reasons: Vec<BTreeSet<VersionLinkReason>> = vec![BTreeSet::new(); n];

    let path_to_idx: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(idx, f)| (f.path_lower.clone(), idx))
        .collect();

    // Signal 1: Same anchor directory (including `<anchor>\Versions\...`) + stem + extension family.
    let mut anchor_buckets: HashMap<(String, String, String, String), Vec<usize>> = HashMap::new();
    for (idx, f) in files.iter().enumerate() {
        if f.parsed.canonical_stem_lower.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
            continue;
        }
        let key = (
            f.volume_id.clone(),
            norm_lower(&f.parsed.anchor_dir),
            f.parsed.canonical_stem_lower.clone(),
            f.parsed.ext_group.clone(),
        );
        anchor_buckets.entry(key).or_default().push(idx);
    }
    for members in anchor_buckets.values() {
        if members.len() < 2 {
            continue;
        }
        let any_versions_dir = members.iter().any(|&i| files[i].parsed.in_versions_folder);
        let any_pattern = members
            .iter()
            .any(|&i| files[i].parsed.has_explicit_version_tag)
            || members
                .iter()
                .map(|&i| files[i].filename.to_ascii_lowercase())
                .collect::<HashSet<_>>()
                .len()
                > 1;
        let anchor_idx = members[0];
        for &m in members {
            dsu.union(anchor_idx, m);
            if any_versions_dir && (files[m].parsed.in_versions_folder || !any_pattern || members.len() >= 2) {
                node_reasons[m].insert(VersionLinkReason::VersionsFolder);
            }
            if any_pattern {
                node_reasons[m].insert(VersionLinkReason::FilenamePattern);
            }
        }
    }

    // Signal 2: Same collection + same canonical stem across subfolders when versioned or in `Versions\`.
    let mut col_buckets: HashMap<(String, String, String, String), Vec<usize>> = HashMap::new();
    for (idx, f) in files.iter().enumerate() {
        let Some(ref cid) = f.collection_id else {
            continue;
        };
        if is_generic_boilerplate_stem(&f.parsed.canonical_stem_lower)
            || f.parsed
                .canonical_stem_lower
                .chars()
                .filter(|c| c.is_alphanumeric())
                .count()
                < 2
        {
            continue;
        }
        let key = (
            f.volume_id.clone(),
            cid.clone(),
            f.parsed.canonical_stem_lower.clone(),
            f.parsed.ext_group.clone(),
        );
        col_buckets.entry(key).or_default().push(idx);
    }
    for members in col_buckets.values() {
        if members.len() < 2 {
            continue;
        }
        let any_versions_dir = members.iter().any(|&i| files[i].parsed.in_versions_folder);
        let any_explicit = members
            .iter()
            .any(|&i| files[i].parsed.has_explicit_version_tag);
        if !any_versions_dir && !any_explicit {
            continue;
        }
        let anchor_idx = members[0];
        for &m in members {
            dsu.union(anchor_idx, m);
            if any_versions_dir {
                node_reasons[m].insert(VersionLinkReason::VersionsFolder);
            }
            if any_explicit {
                node_reasons[m].insert(VersionLinkReason::FilenamePattern);
            }
        }
    }

    // Signal 3: `file_history` lineage linking renamed/moved versions to siblings & recording former paths.
    let history_rows = load_file_history(conn)?;
    let mut former_paths_by_file: HashMap<String, Vec<String>> = HashMap::new();
    let idx_by_file_id: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.file_id.clone(), i))
        .collect();

    for (fid, old_path, _new_path) in history_rows {
        let entry = former_paths_by_file.entry(fid.clone()).or_default();
        if !entry.iter().any(|p| p.eq_ignore_ascii_case(&old_path)) {
            entry.push(old_path.clone());
        }
        let Some(&idx_a) = idx_by_file_id.get(&fid) else {
            continue;
        };
        let old_low = norm_lower(&old_path);
        if let Some(&idx_b) = path_to_idx.get(&old_low)
            && idx_b != idx_a
        {
            dsu.union(idx_a, idx_b);
            node_reasons[idx_a].insert(VersionLinkReason::FileHistory);
            node_reasons[idx_b].insert(VersionLinkReason::FileHistory);
        }

        let mount = vol_mounts
            .get(&files[idx_a].volume_id)
            .map(String::as_str)
            .unwrap_or(&files[idx_a].mount_point);
        let old_name = Path::new(&old_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let old_parsed = parse_version_name(&old_name, &old_path, mount);
        if old_parsed
            .canonical_stem_lower
            .chars()
            .filter(|c| c.is_alphanumeric())
            .count()
            >= 2
        {
            let key = (
                files[idx_a].volume_id.clone(),
                norm_lower(&old_parsed.anchor_dir),
                old_parsed.canonical_stem_lower.clone(),
                old_parsed.ext_group.clone(),
            );
            if let Some(siblings) = anchor_buckets.get(&key) {
                for &sib_idx in siblings {
                    if sib_idx != idx_a {
                        dsu.union(idx_a, sib_idx);
                        node_reasons[idx_a].insert(VersionLinkReason::FileHistory);
                        node_reasons[sib_idx].insert(VersionLinkReason::FileHistory);
                    }
                }
            }
            if let Some(ref cid) = files[idx_a].collection_id
                && !is_generic_boilerplate_stem(&old_parsed.canonical_stem_lower)
            {
                let ckey = (
                    files[idx_a].volume_id.clone(),
                    cid.clone(),
                    old_parsed.canonical_stem_lower.clone(),
                    old_parsed.ext_group.clone(),
                );
                if let Some(col_sibs) = col_buckets.get(&ckey) {
                    for &sib_idx in col_sibs {
                        if sib_idx != idx_a {
                            dsu.union(idx_a, sib_idx);
                            node_reasons[idx_a].insert(VersionLinkReason::FileHistory);
                            node_reasons[sib_idx].insert(VersionLinkReason::FileHistory);
                        }
                    }
                }
            }
        }
    }

    // Signal 4: Semantic vector similarity (`FR-004.2`), strictly honoring `R7` privacy gate.
    let min_sim = options
        .min_similarity
        .unwrap_or(DEFAULT_MIN_SIMILARITY)
        .clamp(0.75, 0.999);
    let eligible_vec_ids: Vec<String> = files
        .iter()
        .filter(|f| !is_sensitive(&f.sensitivity) || reveal_sensitive)
        .map(|f| f.file_id.clone())
        .collect();
    let file_vectors = load_file_vectors(conn, &eligible_vec_ids)?;

    if !file_vectors.is_empty() {
        let mut sem_buckets: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
        for (idx, f) in files.iter().enumerate() {
            if !file_vectors.contains_key(&f.file_id) {
                continue;
            }
            let scope = f
                .collection_id
                .clone()
                .unwrap_or_else(|| norm_lower(&f.parsed.anchor_dir));
            let key = (
                f.volume_id.clone(),
                scope,
                semantic_ext_family(&f.parsed.ext_group).to_string(),
            );
            sem_buckets.entry(key).or_default().push(idx);
        }

        for mut bucket in sem_buckets.into_values() {
            if bucket.len() < 2 {
                continue;
            }
            bucket.sort_by(|&a, &b| {
                files[b]
                    .mtime
                    .cmp(&files[a].mtime)
                    .then_with(|| files[a].file_id.cmp(&files[b].file_id))
            });
            bucket.truncate(MAX_SEMANTIC_BUCKET_FILES);
            for i in 0..bucket.len() {
                let ia = bucket[i];
                let Some(va) = file_vectors.get(&files[ia].file_id) else {
                    continue;
                };
                for &ib in &bucket[(i + 1)..] {
                    let Some(vb) = file_vectors.get(&files[ib].file_id) else {
                        continue;
                    };
                    let sim = cosine_similarity(va, vb);
                    if sim >= min_sim {
                        dsu.union(ia, ib);
                        node_reasons[ia].insert(VersionLinkReason::SemanticSimilarity);
                        node_reasons[ib].insert(VersionLinkReason::SemanticSimilarity);
                    }
                }
            }
        }
    }

    // Materialize connected components with >= 2 nodes.
    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for idx in 0..n {
        let root = dsu.find(idx);
        components.entry(root).or_default().push(idx);
    }

    let query_lower = options
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let only_with_dupes = options.only_with_dupes.unwrap_or(false);
    let include_dismissed = options.include_dismissed.unwrap_or(false);

    let mut dismissed_count = 0usize;
    let mut assembled: Vec<VersionChain> = Vec::new();

    for member_indices in components.into_values() {
        if member_indices.len() < 2 {
            continue;
        }
        let mut sorted = member_indices;
        sorted.sort_by(|&a, &b| {
            files[a]
                .mtime
                .cmp(&files[b].mtime)
                .then_with(|| files[b].parsed.in_versions_folder.cmp(&files[a].parsed.in_versions_folder))
                .then_with(|| files[a].parsed.version_order_hint.cmp(&files[b].parsed.version_order_hint))
                .then_with(|| files[a].size_bytes.cmp(&files[b].size_bytes))
                .then_with(|| files[a].path_lower.cmp(&files[b].path_lower))
        });

        let first = &files[sorted[0]];
        let newest = &files[*sorted.last().unwrap()];

        // Pick canonical metadata from the primary live node (prefer non-Versions anchor if available).
        let primary_live_idx = sorted
            .iter()
            .rev()
            .copied()
            .find(|&i| !files[i].parsed.in_versions_folder)
            .unwrap_or(*sorted.last().unwrap());
        let primary = &files[primary_live_idx];

        let anchor_dir = if let Some(ref croot) = primary.collection_root {
            let has_col_versions = sorted.iter().any(|&i| {
                files[i].parsed.in_versions_folder
                    && norm_lower(&files[i].parsed.anchor_dir) == norm_lower(croot)
            });
            if has_col_versions {
                croot.clone()
            } else {
                primary.parsed.anchor_dir.clone()
            }
        } else {
            primary.parsed.anchor_dir.clone()
        };

        let suggested_versions_dir = join_dir(&anchor_dir, "Versions");
        let chain_key = stable_chain_key(
            &primary.volume_id,
            &anchor_dir,
            &primary.parsed.canonical_stem_lower,
            &primary.parsed.ext_group,
        );

        let pref = prefs.get(&chain_key);
        let dismissed = pref.is_some_and(|p| p.dismissed);
        if dismissed {
            dismissed_count += 1;
            if !include_dismissed {
                continue;
            }
        }

        let pinned_valid = pref
            .and_then(|p| p.pinned_latest_file_id.clone())
            .filter(|pid| sorted.iter().any(|&i| files[i].file_id == *pid));

        let latest_idx = pinned_valid
            .as_ref()
            .and_then(|pid| sorted.iter().copied().find(|&i| files[i].file_id == *pid))
            .unwrap_or(primary_live_idx);
        let latest_file_id = files[latest_idx].file_id.clone();

        // Exact BLAKE3 duplicates within this version chain.
        let mut by_hash: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
        for &idx in &sorted {
            if let Some(ref h) = files[idx].blake3
                && !h.is_empty()
            {
                by_hash.entry(h.clone()).or_default().push(idx);
            }
        }
        let mut duplicate_of: HashMap<usize, String> = HashMap::new();
        for group in by_hash.values() {
            if group.len() < 2 {
                continue;
            }
            let rep_idx = group
                .iter()
                .copied()
                .find(|&i| i == latest_idx)
                .or_else(|| {
                    group
                        .iter()
                        .copied()
                        .rev()
                        .find(|&i| !files[i].parsed.in_versions_folder)
                })
                .unwrap_or(*group.last().unwrap());
            let rep_id = files[rep_idx].file_id.clone();
            for &idx in group {
                if idx != rep_idx {
                    duplicate_of.insert(idx, rep_id.clone());
                }
            }
        }

        let mut chain_reasons_set = BTreeSet::new();
        let mut nodes = Vec::with_capacity(sorted.len());
        let mut exact_duplicate_count = 0usize;
        let mut total_bytes = 0i64;
        let mut reclaimable_duplicate_bytes = 0i64;
        let mut has_sensitive = false;

        for (pos, &idx) in sorted.iter().enumerate() {
            let f = &files[idx];
            total_bytes += f.size_bytes;
            if is_sensitive(&f.sensitivity) {
                has_sensitive = true;
            }

            let former_paths = former_paths_by_file
                .get(&f.file_id)
                .cloned()
                .unwrap_or_default();
            if !former_paths.is_empty() {
                node_reasons[idx].insert(VersionLinkReason::FileHistory);
            }
            let reasons: Vec<VersionLinkReason> = node_reasons[idx].iter().copied().collect();
            for r in &reasons {
                chain_reasons_set.insert(*r);
            }

            let (size_delta_bytes, time_delta_ms, semantic_similarity_prev) = if pos == 0 {
                (None, None, None)
            } else {
                let prev = &files[sorted[pos - 1]];
                let sim = match (
                    file_vectors.get(&prev.file_id),
                    file_vectors.get(&f.file_id),
                ) {
                    (Some(vp), Some(vc)) => Some(cosine_similarity(vp, vc)),
                    _ => None,
                };
                (
                    Some(f.size_bytes - prev.size_bytes),
                    Some(f.mtime - prev.mtime),
                    sim,
                )
            };

            let dup_of = duplicate_of.get(&idx).cloned();
            let is_exact_duplicate = dup_of.is_some();
            if is_exact_duplicate {
                exact_duplicate_count += 1;
                reclaimable_duplicate_bytes += f.size_bytes;
            }

            let is_latest = idx == latest_idx;
            let is_pinned = pinned_valid.as_deref() == Some(f.file_id.as_str());

            nodes.push(VersionNode {
                file_id: f.file_id.clone(),
                path: f.path.clone(),
                filename: f.filename.clone(),
                ext: f.ext.clone(),
                size_bytes: f.size_bytes,
                mtime: f.mtime,
                volume_id: f.volume_id.clone(),
                drive_label: f.drive_label.clone(),
                drive_online: f.drive_online,
                sensitivity: f.sensitivity.clone(),
                version_label: f.parsed.version_label.clone(),
                in_versions_folder: f.parsed.in_versions_folder,
                is_latest,
                is_pinned,
                is_exact_duplicate,
                duplicate_of_file_id: dup_of,
                size_delta_bytes,
                time_delta_ms,
                semantic_similarity_prev,
                former_paths,
                link_reasons: reasons,
            });
        }

        let has_semantic_links = chain_reasons_set.contains(&VersionLinkReason::SemanticSimilarity);
        let has_versions_folder = chain_reasons_set.contains(&VersionLinkReason::VersionsFolder);
        let has_file_history = chain_reasons_set.contains(&VersionLinkReason::FileHistory);
        let link_reasons: Vec<VersionLinkReason> = chain_reasons_set.into_iter().collect();

        if let Some(ref cid_filter) = options.collection_id
            && primary.collection_id.as_deref() != Some(cid_filter.as_str())
        {
            continue;
        }
        if let Some(sig) = options.signal
            && !link_reasons.contains(&sig)
        {
            continue;
        }
        if only_with_dupes && exact_duplicate_count == 0 {
            continue;
        }

        let title = if primary.parsed.ext_group.is_empty() {
            primary.parsed.canonical_stem.clone()
        } else {
            format!("{}.{}", primary.parsed.canonical_stem, primary.ext)
        };

        let mut summary_parts = vec![format!("{} versions", nodes.len())];
        if exact_duplicate_count > 0 {
            summary_parts.push(format!(
                "{exact_duplicate_count} exact duplicate{}",
                if exact_duplicate_count == 1 { "" } else { "s" }
            ));
        }
        if has_semantic_links {
            summary_parts.push("semantic similarity linked".into());
        }
        if has_versions_folder {
            summary_parts.push("includes Versions\\ archive".into());
        }
        summary_parts.push(format!("latest: {}", files[latest_idx].filename));
        let mut summary = summary_parts.join(" · ");
        if dismissed {
            summary.push_str(" (Dismissed)");
        }

        let chain = VersionChain {
            chain_key,
            title,
            canonical_stem: primary.parsed.canonical_stem.clone(),
            ext: primary.ext.clone(),
            collection_id: primary.collection_id.clone(),
            collection_name: primary.collection_name.clone(),
            volume_id: primary.volume_id.clone(),
            drive_label: primary.drive_label.clone(),
            drive_online: primary.drive_online,
            anchor_dir,
            suggested_versions_dir,
            latest_file_id,
            pinned_latest_file_id: pinned_valid,
            version_count: nodes.len(),
            exact_duplicate_count,
            total_bytes,
            reclaimable_duplicate_bytes,
            oldest_mtime: first.mtime,
            newest_mtime: newest.mtime,
            has_semantic_links,
            has_versions_folder,
            has_file_history,
            has_sensitive,
            dismissed,
            link_reasons,
            summary,
            nodes,
        };

        if let Some(ref ql) = query_lower {
            let matches_query = chain.title.to_ascii_lowercase().contains(ql)
                || chain.canonical_stem.to_ascii_lowercase().contains(ql)
                || chain.anchor_dir.to_ascii_lowercase().contains(ql)
                || chain
                    .collection_name
                    .as_deref()
                    .is_some_and(|c| c.to_ascii_lowercase().contains(ql))
                || chain.nodes.iter().any(|n| {
                    n.filename.to_ascii_lowercase().contains(ql)
                        || n.path.to_ascii_lowercase().contains(ql)
                });
            if !matches_query {
                continue;
            }
        }

        assembled.push(chain);
    }

    // Drain `files` drop cleanly.
    files.clear();

    assembled.sort_by(|a, b| {
        a.dismissed
            .cmp(&b.dismissed)
            .then_with(|| (b.exact_duplicate_count > 0).cmp(&(a.exact_duplicate_count > 0)))
            .then_with(|| b.has_semantic_links.cmp(&a.has_semantic_links))
            .then_with(|| b.version_count.cmp(&a.version_count))
            .then_with(|| b.newest_mtime.cmp(&a.newest_mtime))
            .then_with(|| a.title.cmp(&b.title))
    });

    let total_chains = assembled.len();
    let total_versions = assembled.iter().map(|c| c.version_count).sum();
    let chains_with_dupes = assembled.iter().filter(|c| c.exact_duplicate_count > 0).count();
    let reclaimable_duplicate_bytes = assembled
        .iter()
        .map(|c| c.reclaimable_duplicate_bytes)
        .sum();
    let semantic_chains_count = assembled.iter().filter(|c| c.has_semantic_links).count();
    let versions_folder_chains_count = assembled.iter().filter(|c| c.has_versions_folder).count();

    let limit = options.limit.unwrap_or(100).clamp(1, 500);
    assembled.truncate(limit);

    Ok(VersionScanReport {
        scanned_at: now_ms,
        total_chains,
        total_versions,
        chains_with_dupes,
        reclaimable_duplicate_bytes,
        semantic_chains_count,
        versions_folder_chains_count,
        dismissed_count,
        chains: assembled,
    })
}

/// Pins a specific live `file_id` as the authoritative latest version for `chain_key`.
pub fn pin_latest_version(conn: &Connection, chain_key: &str, file_id: &str) -> Result<()> {
    let ck = chain_key.trim();
    if ck.is_empty() {
        return Err(AegisError::InvalidInput("chain_key must not be empty.".into()));
    }
    let live: bool = conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM files f JOIN volumes v ON v.id = f.volume_id
           WHERE f.id = ?1 AND f.deleted_at IS NULL
             AND f.state NOT IN ('quarantine', 'missing')
             AND COALESCE(v.role, '') != 'backup'
         )",
        params![file_id],
        |r| r.get(0),
    )?;
    if !live {
        return Err(AegisError::NotFound(format!("live file {file_id}")));
    }
    let now = now_ms();
    conn.execute(
        "INSERT INTO version_chain_prefs (chain_key, pinned_latest_file_id, dismissed, dismissed_at, updated_at)
         VALUES (?1, ?2, 0, NULL, ?3)
         ON CONFLICT(chain_key) DO UPDATE SET
           pinned_latest_file_id = excluded.pinned_latest_file_id,
           updated_at = excluded.updated_at",
        params![ck, file_id, now],
    )?;
    Ok(())
}

/// Persists dismissal for a version chain so it hides from default scans.
pub fn dismiss_version_chain(conn: &Connection, chain_key: &str) -> Result<()> {
    let ck = chain_key.trim();
    if ck.is_empty() {
        return Err(AegisError::InvalidInput("chain_key must not be empty.".into()));
    }
    let now = now_ms();
    conn.execute(
        "INSERT INTO version_chain_prefs (chain_key, pinned_latest_file_id, dismissed, dismissed_at, updated_at)
         VALUES (?1, NULL, 1, ?2, ?2)
         ON CONFLICT(chain_key) DO UPDATE SET
           dismissed = 1,
           dismissed_at = excluded.dismissed_at,
           updated_at = excluded.updated_at",
        params![ck, now],
    )?;
    Ok(())
}

/// Restores a previously dismissed version chain back into active scans.
pub fn undismiss_version_chain(conn: &Connection, chain_key: &str) -> Result<()> {
    let ck = chain_key.trim();
    if ck.is_empty() {
        return Err(AegisError::InvalidInput("chain_key must not be empty.".into()));
    }
    let now = now_ms();
    conn.execute(
        "UPDATE version_chain_prefs SET dismissed = 0, dismissed_at = NULL, updated_at = ?2 WHERE chain_key = ?1",
        params![ck, now],
    )?;
    Ok(())
}

fn load_volume_mounts(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut st = conn.prepare("SELECT id, mount_point FROM volumes")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn load_collection_locations(conn: &Connection) -> Result<Vec<ActiveCollectionLoc>> {
    let mut st = conn.prepare(
        "SELECT c.id, c.name, cl.volume_id, cl.root_path
         FROM collections c
         JOIN collection_locations cl ON cl.collection_id = c.id
         WHERE c.state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    let rows = st.query_map([], |r| {
        Ok(ActiveCollectionLoc {
            id: r.get(0)?,
            name: r.get(1)?,
            volume_id: r.get(2)?,
            root_path: r.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

struct MemberCollectionInfo {
    id: String,
    name: String,
    root_path: Option<String>,
}

fn load_collection_members(conn: &Connection) -> Result<HashMap<String, MemberCollectionInfo>> {
    let mut st = conn.prepare(
        "SELECT cm.file_id, c.id, c.name,
                (SELECT cl.root_path FROM collection_locations cl
                 WHERE cl.collection_id = c.id LIMIT 1) AS root_path
         FROM collection_members cm
         JOIN collections c ON c.id = cm.collection_id
         WHERE c.state NOT IN ('left_as_is', 'not_a_project')",
    )?;
    let mut out = HashMap::new();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            MemberCollectionInfo {
                id: r.get(1)?,
                name: r.get(2)?,
                root_path: r.get(3)?,
            },
        ))
    })?;
    for row in rows {
        let (fid, info) = row?;
        out.insert(fid, info);
    }
    Ok(out)
}

fn load_chain_prefs(conn: &Connection) -> Result<HashMap<String, ChainPref>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'version_chain_prefs')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(HashMap::new());
    }
    let mut st = conn.prepare(
        "SELECT chain_key, pinned_latest_file_id, dismissed FROM version_chain_prefs",
    )?;
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            ChainPref {
                pinned_latest_file_id: r.get(1)?,
                dismissed: r.get::<_, i64>(2)? != 0,
            },
        ))
    })?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn load_file_history(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut st = conn.prepare(
        "SELECT file_id, old_path, new_path FROM file_history ORDER BY changed_at ASC, rowid ASC",
    )?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

fn match_collection(
    file_id: &str,
    volume_id: &str,
    path: &str,
    locations: &[ActiveCollectionLoc],
    members: &HashMap<String, MemberCollectionInfo>,
) -> Option<(String, String, Option<String>)> {
    if let Some(info) = members.get(file_id) {
        return Some((info.id.clone(), info.name.clone(), info.root_path.clone()));
    }
    let mut best: Option<(usize, &ActiveCollectionLoc)> = None;
    for loc in locations {
        if loc.volume_id != volume_id {
            continue;
        }
        if path_is_within(path, &loc.root_path, false) {
            let len = loc.root_path.len();
            if best.as_ref().is_none_or(|(blen, _)| len > *blen) {
                best = Some((len, loc));
            }
        }
    }
    best.map(|(_, l)| (l.id.clone(), l.name.clone(), Some(l.root_path.clone())))
}

fn load_candidate_files(
    conn: &Connection,
    volume_id_filter: Option<&str>,
    collections: &[ActiveCollectionLoc],
    members: &HashMap<String, MemberCollectionInfo>,
) -> Result<Vec<DbFileCandidate>> {
    let base_sql = "SELECT f.id, f.volume_id, v.mount_point,
                           COALESCE(NULLIF(TRIM(v.label), ''), v.mount_point, v.id) AS drive_label,
                           COALESCE(v.status = 'online', 0) AS drive_online,
                           f.path, f.name, COALESCE(f.ext, ''), f.size, f.mtime, f.blake3, f.sensitivity
                    FROM files f
                    JOIN volumes v ON v.id = f.volume_id
                    WHERE f.deleted_at IS NULL
                      AND f.state NOT IN ('quarantine', 'missing')
                      AND f.container_file_id IS NULL
                      AND COALESCE(v.role, '') != 'backup'";

    let mut out = Vec::new();
    let map_row = |r: &rusqlite::Row<'_>| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)? != 0,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, i64>(8)?,
            r.get::<_, i64>(9)?,
            r.get::<_, Option<Vec<u8>>>(10)?,
            r.get::<_, String>(11)?,
        ))
    };

    let rows: Vec<_> = if let Some(vid) = volume_id_filter.map(str::trim).filter(|s| !s.is_empty()) {
        let sql = format!("{base_sql} AND f.volume_id = ?1");
        let mut st = conn.prepare(&sql)?;
        let mapped = st.query_map(params![vid], map_row)?;
        mapped.collect::<rusqlite::Result<_>>()?
    } else {
        let mut st = conn.prepare(base_sql)?;
        let mapped = st.query_map([], map_row)?;
        mapped.collect::<rusqlite::Result<_>>()?
    };

    for (file_id, volume_id, mount_point, drive_label, drive_online, path, filename, ext, size_bytes, mtime, blake3, sensitivity) in rows {
        if is_excluded_relative_path(&path, &mount_point) {
            continue;
        }
        let parsed = parse_version_name(&filename, &path, &mount_point);
        if parsed.canonical_stem_lower.is_empty() {
            continue;
        }
        let col = match_collection(&file_id, &volume_id, &path, collections, members);
        let (collection_id, collection_name, collection_root) = match col {
            Some((id, name, root)) => (Some(id), Some(name), root),
            None => (None, None, None),
        };
        let path_lower = norm_lower(&path);
        out.push(DbFileCandidate {
            file_id,
            volume_id,
            mount_point,
            drive_label,
            drive_online,
            path,
            path_lower,
            filename,
            ext,
            size_bytes,
            mtime,
            blake3,
            sensitivity,
            parsed,
            collection_id,
            collection_name,
            collection_root,
        });
    }
    Ok(out)
}
