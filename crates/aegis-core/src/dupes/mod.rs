//! Exact duplicates across every drive, grouped so each question covers a folder or a batch
//! of files rather than one file (spec §7.5).

pub mod decisions;
pub mod safety;

pub use crate::contracts::{DupeGroup, DupeGroupKind, DupeLocation, DupeMember, GroupedDupeReport};
pub use safety::{MIN_COPIES, copy_count, verify_survivor};

use crate::error::Result;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Share of files two folders must have in common (each way) to be "nearly the same".
pub const PARTIAL_OVERLAP: f64 = 0.90;
const JUNK_NAMES: [&str; 3] = ["thumbs.db", "desktop.ini", ".ds_store"];

#[derive(Debug, Clone)]
struct Vol {
    mount: String,
    label: Option<String>,
    online: bool,
    is_backup: bool,
    identity: String,
    last_seen: i64,
}

#[derive(Debug, Clone)]
struct FileRow {
    id: String,
    volume_id: String,
    path: String,
    size: i64,
    mtime: i64,
    hash: Option<Vec<u8>>,
    file_index: Option<Vec<u8>>,
}

/// A folder on a volume: `(volume id, lowercase path with '/' separators)`.
type DirKey = (String, String);
/// Volumes, live files, `(backup volume, rel path)` of recorded backup copies, and the
/// location pairs the owner answered "keep both" for.
type Catalog = (HashMap<String, Vol>, Vec<FileRow>, HashSet<(String, String)>, HashSet<(String, String)>);
/// Files under a folder keyed by lowercase relative path → (file index, hash).
type RelSet = BTreeMap<String, (usize, Option<Vec<u8>>)>;

#[derive(Debug, Clone, Default)]
struct DirStat {
    display: String,
    count: i64,
    sig: [u64; 4],
    unmatchable: bool,
    has_dup: bool,
}

fn norm(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn parent_display(path: &str) -> Option<&str> {
    let cut = path.rfind(['\\', '/'])?;
    if cut == 0 {
        return None;
    }
    // Keep the separator for drive roots ("E:\").
    if path[..cut].ends_with(':') {
        Some(&path[..=cut])
    } else {
        Some(&path[..cut])
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn is_junk(path: &str) -> bool {
    JUNK_NAMES.contains(&file_name(path).to_lowercase().as_str())
}

/// Path of `path` relative to the folder `dir` ('/'-separated, original case).
fn rel_to(path: &str, dir: &str) -> String {
    path.get(dir.len()..)
        .unwrap_or("")
        .trim_start_matches(['\\', '/'])
        .replace('\\', "/")
}

/// Folders from the file's parent up to (and including) its volume's mount point.
fn ancestors<'a>(path: &'a str, mount: &str) -> Vec<&'a str> {
    let mount_n = norm(mount);
    let mut out = Vec::new();
    let mut cur = parent_display(path);
    while let Some(d) = cur {
        let dn = norm(d);
        if dn.len() < mount_n.len() || !(dn == mount_n || dn.starts_with(&format!("{mount_n}/"))) {
            break;
        }
        out.push(d);
        if dn == mount_n {
            break;
        }
        cur = parent_display(d);
    }
    out
}

fn add_sig(sig: &mut [u64; 4], rel: &str, hash: &[u8]) {
    let mut h = blake3::Hasher::new();
    h.update(rel.to_lowercase().as_bytes());
    h.update(&[0]);
    h.update(hash);
    let out = h.finalize();
    let b = out.as_bytes();
    for (i, lane) in sig.iter_mut().enumerate() {
        *lane = lane.wrapping_add(u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap()));
    }
}

fn load(conn: &Connection) -> Result<Catalog> {
    let mut vols = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT id, mount_point, label, status, role, passport_id, last_seen FROM volumes",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Vol {
                mount: r.get(1)?,
                label: r.get(2)?,
                online: r.get::<_, String>(3)? == "online",
                is_backup: r.get::<_, String>(4)? == "backup",
                identity: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                last_seen: r.get(6)?,
            },
        ))
    })?;
    for r in rows {
        let (id, mut v) = r?;
        if v.identity.is_empty() {
            v.identity = id.clone();
        }
        vols.insert(id, v);
    }
    let mut stmt = conn.prepare(
        "SELECT id, volume_id, path, size, mtime, blake3, file_index FROM files
         WHERE deleted_at IS NULL AND state NOT IN ('quarantine','missing')
           AND size > 0 AND container_file_id IS NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FileRow {
            id: r.get(0)?,
            volume_id: r.get(1)?,
            path: r.get(2)?,
            size: r.get(3)?,
            mtime: r.get(4)?,
            hash: r.get(5)?,
            file_index: r.get(6)?,
        })
    })?;
    let files = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let mut copies = HashSet::new();
    let mut stmt = conn.prepare("SELECT backup_volume_id, backup_rel_path FROM backup_copies")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for r in rows {
        let (v, rel) = r?;
        copies.insert((v, norm(&rel)));
    }
    let mut keep_both = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT a_passport, a_rel_path, b_passport, b_rel_path FROM dupe_decisions
         WHERE decision = 'keep_both' AND undone_at IS NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            format!("{}|{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?),
            format!("{}|{}", r.get::<_, String>(2)?, r.get::<_, String>(3)?),
        ))
    })?;
    for r in rows {
        keep_both.insert(pair_key(r?));
    }
    Ok((vols, files, copies, keep_both))
}

/// Order-independent key for a pair of location identities.
fn pair_key((a, b): (String, String)) -> (String, String) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Stable identity of a location: `"<passport or volume id>|<path relative to the mount>"`.
pub fn location_identity(conn: &Connection, loc: &DupeLocation) -> Result<String> {
    let (identity, mount): (Option<String>, String) = conn.query_row(
        "SELECT passport_id, mount_point FROM volumes WHERE id = ?1",
        [&loc.volume_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(format!(
        "{}|{}",
        identity.unwrap_or_else(|| loc.volume_id.clone()),
        rel_to_mount(&loc.path, &mount)
    ))
}

/// Builds the grouped duplicate report over the whole catalog.
pub fn grouped_report(conn: &Connection) -> Result<GroupedDupeReport> {
    let (vols, files, backup_copies, keep_both) = load(conn)?;

    // Duplicate hashes: ≥2 distinct copies (hardlinks count once).
    let mut by_hash: HashMap<&[u8], Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        if let Some(h) = f.hash.as_deref()
            && vols.contains_key(&f.volume_id)
            && !is_junk(&f.path)
        {
            by_hash.entry(h).or_default().push(i);
        }
    }
    by_hash.retain(|_, members| {
        let mut seen = HashSet::new();
        members.retain(|&i| {
            let f = &files[i];
            seen.insert((f.volume_id.clone(), f.file_index.clone().unwrap_or_else(|| f.id.clone().into_bytes())))
        });
        members.len() >= 2
    });
    let dup_file: HashSet<usize> = by_hash.values().flatten().copied().collect();

    // Folder statistics for every ancestor of a duplicate.
    let mut candidate: HashSet<DirKey> = HashSet::new();
    for &i in &dup_file {
        let f = &files[i];
        for d in ancestors(&f.path, &vols[&f.volume_id].mount) {
            candidate.insert((f.volume_id.clone(), norm(d)));
        }
    }
    let mut dirs: HashMap<DirKey, DirStat> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        let Some(v) = vols.get(&f.volume_id) else { continue };
        if is_junk(&f.path) {
            continue;
        }
        for d in ancestors(&f.path, &v.mount) {
            let key = (f.volume_id.clone(), norm(d));
            if !candidate.contains(&key) {
                continue;
            }
            let st = dirs.entry(key).or_insert_with(|| DirStat {
                display: d.to_string(),
                ..Default::default()
            });
            st.count += 1;
            match &f.hash {
                Some(h) => add_sig(&mut st.sig, &rel_to(&f.path, d), h),
                None => st.unmatchable = true,
            }
            if dup_file.contains(&i) {
                st.has_dup = true;
            }
        }
    }

    // Exact folder groups: same signature and file count.
    let mut by_sig: HashMap<([u64; 4], i64), Vec<DirKey>> = HashMap::new();
    for (k, st) in &dirs {
        if st.count >= 2 && !st.unmatchable && st.has_dup {
            by_sig.entry((st.sig, st.count)).or_default().push(k.clone());
        }
    }
    let exact_groups: Vec<Vec<DirKey>> = by_sig
        .into_values()
        .filter(|g| g.len() >= 2)
        .map(|mut g| {
            g.sort();
            g
        })
        .collect();

    // Folder pairs sharing files at the same relative paths.
    let mut pair_matches: HashMap<(DirKey, DirKey), i64> = HashMap::new();
    for members in by_hash.values() {
        for (x, &i) in members.iter().enumerate() {
            for &j in &members[x + 1..] {
                let (fa, fb) = (&files[i], &files[j]);
                let anc_a = ancestors(&fa.path, &vols[&fa.volume_id].mount);
                let anc_b = ancestors(&fb.path, &vols[&fb.volume_id].mount);
                let (mut name_a, mut name_b) = (file_name(&fa.path), file_name(&fb.path));
                for (da, db) in anc_a.iter().zip(anc_b.iter()) {
                    if !name_a.eq_ignore_ascii_case(name_b) {
                        break;
                    }
                    let ka = (fa.volume_id.clone(), norm(da));
                    let kb = (fb.volume_id.clone(), norm(db));
                    let nested = ka.0 == kb.0
                        && (ka.1 == kb.1 || ka.1.starts_with(&format!("{}/", kb.1)) || kb.1.starts_with(&format!("{}/", ka.1)));
                    if nested {
                        break;
                    }
                    let key = if ka <= kb { (ka, kb) } else { (kb, ka) };
                    *pair_matches.entry(key).or_insert(0) += 1;
                    name_a = file_name(da);
                    name_b = file_name(db);
                }
            }
        }
    }

    // Every qualifying pair (exact or partial), for "report only the topmost" suppression.
    let mut exact_pairs: HashSet<(DirKey, DirKey)> = HashSet::new();
    for g in &exact_groups {
        for (x, a) in g.iter().enumerate() {
            for b in &g[x + 1..] {
                exact_pairs.insert((a.clone(), b.clone()));
            }
        }
    }
    let mut partial_pairs: Vec<(DirKey, DirKey)> = Vec::new();
    for ((a, b), m) in &pair_matches {
        if exact_pairs.contains(&(a.clone(), b.clone())) {
            continue;
        }
        let (Some(sa), Some(sb)) = (dirs.get(a), dirs.get(b)) else { continue };
        if sa.count < 2 || sb.count < 2 {
            continue;
        }
        let share_a = *m as f64 / sa.count as f64;
        let share_b = *m as f64 / sb.count as f64;
        if share_a >= PARTIAL_OVERLAP && share_b >= PARTIAL_OVERLAP {
            partial_pairs.push((a.clone(), b.clone()));
        }
    }
    let qualifying: HashSet<(DirKey, DirKey)> =
        exact_pairs.iter().cloned().chain(partial_pairs.iter().cloned()).collect();
    let parent_key = |k: &DirKey| -> Option<DirKey> {
        let disp = &dirs.get(k)?.display;
        let mount = norm(&vols[&k.0].mount);
        if k.1 == mount {
            return None;
        }
        parent_display(disp).map(|p| (k.0.clone(), norm(p)))
    };
    let suppressed = |a: &DirKey, b: &DirKey| -> bool {
        let (Some(pa), Some(pb)) = (parent_key(a), parent_key(b)) else { return false };
        let same_name = file_name(&a.1) == file_name(&b.1);
        let key = if pa <= pb { (pa, pb) } else { (pb, pa) };
        same_name && qualifying.contains(&key)
    };

    let mut groups: Vec<DupeGroup> = Vec::new();
    let mut covered: Vec<DirKey> = Vec::new();
    let location_of = |k: &DirKey| -> DupeLocation {
        let v = &vols[&k.0];
        DupeLocation {
            file_id: None,
            volume_id: k.0.clone(),
            volume_label: v.label.clone(),
            online: v.online,
            path: dirs[k].display.clone(),
            is_backup: v.is_backup,
            last_seen: v.last_seen,
        }
    };
    let files_under = |k: &DirKey| -> Vec<usize> {
        let prefix = format!("{}/", k.1);
        files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.volume_id == k.0 && norm(&f.path).starts_with(&prefix) && !is_junk(&f.path))
            .map(|(i, _)| i)
            .collect()
    };

    for g in &exact_groups {
        let all_suppressed = g
            .iter()
            .enumerate()
            .all(|(x, a)| g.iter().skip(x + 1).all(|b| suppressed(a, b)));
        if all_suppressed {
            continue;
        }
        let mut members = Vec::new();
        let mut size_bytes = 0;
        for (li, k) in g.iter().enumerate() {
            for i in files_under(k) {
                let f = &files[i];
                if li == 0 {
                    size_bytes += f.size;
                }
                members.push(member(li, f, &dirs[k].display));
            }
        }
        covered.extend(g.iter().cloned());
        groups.push(finish_group(
            DupeGroupKind::Folder,
            g.iter().map(&location_of).collect(),
            members,
            size_bytes,
            dirs[&g[0]].count,
            Vec::new(),
            &vols,
            &files,
            &dirs,
            g,
            &backup_copies,
            &keep_both,
        ));
    }

    partial_pairs.sort();
    for (a, b) in &partial_pairs {
        if suppressed(a, b) {
            continue;
        }
        let pair = vec![a.clone(), b.clone()];
        let rel_sets: Vec<RelSet> = pair
            .iter()
            .map(|k| {
                files_under(k)
                    .into_iter()
                    .map(|i| (rel_to(&files[i].path, &dirs[k].display).to_lowercase(), (i, files[i].hash.clone())))
                    .collect()
            })
            .collect();
        let mut differences = Vec::new();
        let mut members = Vec::new();
        let mut size_bytes = 0;
        let mut matched = 0;
        for (side, other) in [(0usize, 1usize), (1, 0)] {
            for (rel, (i, h)) in &rel_sets[side] {
                let same = rel_sets[other].get(rel).is_some_and(|(_, oh)| oh.is_some() && oh == h);
                if same {
                    members.push(member(side, &files[*i], &dirs[&pair[side]].display));
                    if side == 0 {
                        size_bytes += files[*i].size;
                        matched += 1;
                    }
                } else {
                    differences.push(rel_to(&files[*i].path, &dirs[&pair[side]].display));
                }
            }
        }
        covered.extend(pair.iter().cloned());
        groups.push(finish_group(
            DupeGroupKind::PartialFolder,
            pair.iter().map(&location_of).collect(),
            members,
            size_bytes,
            matched,
            differences,
            &vols,
            &files,
            &dirs,
            &pair,
            &backup_copies,
            &keep_both,
        ));
    }

    // Remaining duplicates, batched by the set of folders they sit in.
    let is_covered = |f: &FileRow| {
        let n = norm(&f.path);
        covered.iter().any(|(v, d)| *v == f.volume_id && n.starts_with(&format!("{d}/")))
    };
    let mut batches: BTreeMap<Vec<DirKey>, Vec<Vec<usize>>> = BTreeMap::new();
    let mut hash_list: Vec<(&&[u8], &Vec<usize>)> = by_hash.iter().collect();
    hash_list.sort_by(|a, b| a.0.cmp(b.0));
    for (_, members) in hash_list {
        if members.iter().all(|&i| is_covered(&files[i])) {
            continue;
        }
        let mut parents: Vec<DirKey> = members
            .iter()
            .map(|&i| {
                let f = &files[i];
                (f.volume_id.clone(), norm(parent_display(&f.path).unwrap_or(&f.path)))
            })
            .collect();
        parents.sort();
        let distinct = parents.windows(2).all(|w| w[0] != w[1]);
        let key = if distinct {
            parents
        } else {
            // Copies side by side in one folder: a group of their own, keyed by the files.
            let mut k: Vec<DirKey> = members
                .iter()
                .map(|&i| (files[i].volume_id.clone(), norm(&files[i].path)))
                .collect();
            k.sort();
            k
        };
        batches.entry(key).or_default().push(members.clone());
    }
    for (key, hashes) in batches {
        let single_files = key.iter().all(|k| !dirs.contains_key(k));
        let mut locations: Vec<DupeLocation> = Vec::new();
        let mut members = Vec::new();
        let mut size_bytes = 0;
        for (li, k) in key.iter().enumerate() {
            let v = &vols[&k.0];
            let (path, file_id) = if single_files {
                let i = hashes[0]
                    .iter()
                    .copied()
                    .find(|&i| files[i].volume_id == k.0 && norm(&files[i].path) == k.1)
                    .unwrap_or(hashes[0][li]);
                (files[i].path.clone(), Some(files[i].id.clone()))
            } else {
                let disp = dirs.get(k).map(|d| d.display.clone()).unwrap_or_else(|| k.1.clone());
                let fid = (hashes.len() == 1)
                    .then(|| hashes[0].iter().find(|&&i| files[i].volume_id == k.0 && norm(parent_display(&files[i].path).unwrap_or("")) == k.1))
                    .flatten()
                    .map(|&i| files[i].id.clone());
                (disp, fid)
            };
            locations.push(DupeLocation {
                file_id,
                volume_id: k.0.clone(),
                volume_label: v.label.clone(),
                online: v.online,
                path,
                is_backup: v.is_backup,
                last_seen: v.last_seen,
            });
        }
        for h in &hashes {
            size_bytes += files[h[0]].size;
            for &i in h {
                let f = &files[i];
                let li = key
                    .iter()
                    .position(|k| {
                        k.0 == f.volume_id
                            && (k.1 == norm(&f.path) || k.1 == norm(parent_display(&f.path).unwrap_or("")))
                    })
                    .unwrap_or(0);
                let base = if single_files {
                    parent_display(&f.path).unwrap_or("").to_string()
                } else {
                    locations[li].path.clone()
                };
                members.push(member(li, f, &base));
            }
        }
        let file_count = hashes.len() as i64;
        groups.push(finish_group(
            DupeGroupKind::Files,
            locations,
            members,
            size_bytes,
            file_count,
            Vec::new(),
            &vols,
            &files,
            &dirs,
            &key,
            &backup_copies,
            &keep_both,
        ));
    }

    groups.sort_by(|a, b| {
        b.size_bytes
            .saturating_mul(b.locations.len() as i64 - 1)
            .cmp(&a.size_bytes.saturating_mul(a.locations.len() as i64 - 1))
            .then_with(|| a.group_id.cmp(&b.group_id))
    });
    let reclaimable_bytes = groups
        .iter()
        .filter(|g| g.needs_question)
        .map(|g| g.size_bytes * (g.locations.len() as i64 - 1))
        .sum();
    Ok(GroupedDupeReport { groups, reclaimable_bytes })
}

fn member(location_index: usize, f: &FileRow, base: &str) -> DupeMember {
    DupeMember {
        location_index,
        file_id: f.id.clone(),
        rel_path: rel_to(&f.path, base),
        blake3_hex: f.hash.as_deref().map(hex).unwrap_or_default(),
        size: f.size,
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Path of `path` relative to its volume's mount point, lowercase with '/' separators.
fn rel_to_mount(path: &str, mount: &str) -> String {
    norm(&rel_to(path, mount))
}

#[allow(clippy::too_many_arguments)]
fn finish_group(
    kind: DupeGroupKind,
    locations: Vec<DupeLocation>,
    members: Vec<DupeMember>,
    size_bytes: i64,
    file_count: i64,
    differences: Vec<String>,
    vols: &HashMap<String, Vol>,
    files: &[FileRow],
    dirs: &HashMap<DirKey, DirStat>,
    keys: &[DirKey],
    backup_copies: &HashSet<(String, String)>,
    keep_both: &HashSet<(String, String)>,
) -> DupeGroup {
    let by_id: HashMap<&str, &FileRow> = files.iter().map(|f| (f.id.as_str(), f)).collect();
    // Keeper (spec §7.5): not on the Backup drive → folder with more siblings → oldest → shortest path.
    let keeper_index = (0..locations.len())
        .min_by_key(|&li| {
            let loc = &locations[li];
            let siblings = keys
                .get(li)
                .and_then(|k| {
                    dirs.get(k).or_else(|| {
                        parent_display(&loc.path).and_then(|p| dirs.get(&(k.0.clone(), norm(p))))
                    })
                })
                .map(|d| d.count)
                .unwrap_or(0);
            let oldest = members
                .iter()
                .filter(|m| m.location_index == li)
                .filter_map(|m| by_id.get(m.file_id.as_str()).map(|f| f.mtime))
                .min()
                .unwrap_or(i64::MAX);
            (loc.is_backup, -siblings, oldest, loc.path.len())
        })
        .unwrap_or(0);

    // A copy already recorded inside "Aegis Backup" on the Backup drive is healthy: no question.
    let healthy_backup = locations.iter().enumerate().any(|(li, loc)| {
        if !loc.is_backup {
            return false;
        }
        let mount = &vols[&loc.volume_id].mount;
        let in_backup_area = rel_to_mount(&loc.path, mount).starts_with("aegis backup");
        let at_loc: Vec<&DupeMember> = members.iter().filter(|m| m.location_index == li).collect();
        in_backup_area
            && !at_loc.is_empty()
            && at_loc.iter().all(|m| {
                by_id.get(m.file_id.as_str()).is_some_and(|f| {
                    backup_copies.contains(&(loc.volume_id.clone(), rel_to_mount(&f.path, mount)))
                })
            })
    });

    let loc_ids: Vec<String> = locations
        .iter()
        .map(|l| {
            let v = &vols[&l.volume_id];
            format!("{}|{}", v.identity, rel_to_mount(&l.path, &v.mount))
        })
        .collect();
    // "Keep both" answered for the kept copy against every other location.
    let answered = locations.len() >= 2
        && (0..locations.len())
            .filter(|&i| i != keeper_index)
            .all(|i| keep_both.contains(&pair_key((loc_ids[keeper_index].clone(), loc_ids[i].clone()))));
    let mut ids = loc_ids.clone();
    ids.sort();
    let group_id = blake3::hash(format!("{kind:?}\n{}", ids.join("\n")).as_bytes())
        .to_hex()[..32]
        .to_string();

    DupeGroup {
        group_id,
        kind,
        size_bytes,
        file_count,
        locations,
        keeper_index,
        differences,
        needs_question: !healthy_backup && !answered,
        members,
    }
}

/// `true` when `path` lies inside `dir` (either separator, case-insensitive).
pub fn path_in(path: &str, dir: &str) -> bool {
    let (p, d) = (norm(path), norm(dir));
    p == d || p.starts_with(&format!("{d}/"))
}
