//! The recognition pass (spec §7, §13): every project on every root and catalogued drive (the
//! Backup drive excepted), copies of one project grouped into one collection, each copy related
//! to the project's home, the owner's answers applied, and the result saved.

use super::profile::profiles_for;
use super::projects::{PROJECT_THRESHOLD, ProjectCandidate, find_projects_with};
use super::relate::{
    ContentSummary, Identity, answered_same, choose_home, content_summaries, fingerprint, fingerprints_match, identity_of,
    normalized_name, relate,
    version_series,
};
use super::store::{decisions, save_collections};
use super::{Collection, CollectionKind, Decision, DecisionKind, Location, Relationship};
use crate::db::SqliteStore;
use crate::db::files::path_is_within;
use crate::error::Result;
use crate::{new_id, now_ms};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;
use ts_rs::TS;

/// A file (or a declared name) found in projects of this many different names is boilerplate,
/// not evidence of identity.
pub const BOILERPLATE_NAMES: usize = 3;

fn hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Settings key recording when recognition last ran (scope `context`).
pub const RECOGNISED_AT: &str = "recognised_at";

/// What a recognition pass found.
#[derive(TS, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[ts(export)]
pub struct RecogniseReport {
    pub projects: usize,
    /// Projects found in more than one place.
    pub multi_location: usize,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// Where recognition looks: the PC's enabled roots, and the mount point of every catalogued
/// drive that isn't the Backup drive and doesn't hold the PC roots (each limited to its volume).
pub fn recognition_roots(store: &SqliteStore) -> Result<Vec<(String, Option<String>)>> {
    let pc: Vec<String> = store.list_roots()?.into_iter().filter(|r| r.enabled).map(|r| r.path).collect();
    let mut out: Vec<(String, Option<String>)> = pc.iter().map(|r| (r.clone(), None)).collect();
    let mut stmt = store.conn().prepare(
        "SELECT v.id, v.mount_point FROM volumes v
         WHERE COALESCE(v.role, 'unassigned') != 'backup'
           AND EXISTS (SELECT 1 FROM files f WHERE f.volume_id = v.id AND f.deleted_at IS NULL)",
    )?;
    let vols: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    for (id, mount) in vols {
        if pc.iter().any(|r| path_is_within(r, &mount, false)) {
            continue;
        }
        out.push((mount, Some(id)));
    }
    Ok(out)
}

/// The owner's latest answer of `kind` about a location with fingerprint `fp`.
fn answer_for<'a>(decisions: &'a [Decision], kind: DecisionKind, fp: &str) -> Option<&'a Decision> {
    decisions
        .iter()
        .filter(|d| d.kind == kind && d.fingerprint_b.is_none() && fingerprints_match(&d.fingerprint_a, fp))
        .max_by_key(|d| d.at)
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r {
        r = parent[r];
    }
    let mut c = i;
    while parent[c] != r {
        let n = parent[c];
        parent[c] = r;
        c = n;
    }
    r
}

/// Runs a full recognition pass and saves what it found (replacing the previous pass).
pub fn recognise(store: &mut SqliteStore) -> Result<RecogniseReport> {
    let t = Instant::now();
    let roots = recognition_roots(store)?;
    let mut cands: Vec<ProjectCandidate> = Vec::new();
    for (root, volume) in &roots {
        let ps = profiles_for(store, &[(root.clone(), volume.clone())])?;
        // A drive's top level is a general place, like Documents: it lends no project status.
        let general: Vec<String> = volume.iter().map(|_| root.clone()).collect();
        cands.extend(find_projects_with(&ps, std::slice::from_ref(root), &general));
    }
    let answers = decisions(store.conn())?;
    let online: HashMap<String, bool> = {
        let mut stmt = store.conn().prepare("SELECT id, status = 'online' FROM volumes")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };

    let locs: Vec<(String, String)> = cands.iter().map(|c| (c.volume_id.clone(), c.root.clone())).collect();
    let summaries: Vec<ContentSummary> = content_summaries(store, &roots, &locs)?;
    let fps: Vec<String> = cands.iter().zip(&summaries).map(|(c, s)| fingerprint(c.name(), &c.declared_names, s, &c.volume_id, &c.root)).collect();

    // "Not a project" drops a folder; "yes" confirms it.
    let keep: Vec<usize> = (0..cands.len())
        .filter(|&i| answer_for(&answers, DecisionKind::IsProject, &fps[i]).is_none_or(|d| d.value != "no"))
        .collect();

    // Copies of one project, joined pairwise. Only pairs that share something worth checking (a
    // name, a declared name, a file, or an answer from the owner) are compared.
    let mut index: HashMap<String, Vec<usize>> = HashMap::new();
    for &i in &keep {
        let mut keys: Vec<String> = vec![format!("n:{}", normalized_name(cands[i].name()))];
        keys.extend(cands[i].declared_names.iter().map(|d| format!("d:{}", d.to_lowercase())));
        for (rel, f) in &summaries[i].files {
            keys.push(format!("p:{rel}:{}", f.size));
            if let Some(h) = &f.blake3 {
                keys.push(format!("h:{}", hex(h)));
            }
        }
        keys.sort();
        keys.dedup();
        for k in keys {
            index.entry(k).or_default().push(i);
        }
    }
    // Boilerplate: a file found in projects of four or more different names (a starter kit, the
    // same agent files, a build wrapper). It says nothing about which project a copy belongs to,
    // so identity is judged without it; relationships between real copies still count it.
    let names: Vec<String> = cands.iter().map(|c| normalized_name(c.name())).collect();
    let boilerplate: std::collections::HashSet<&str> = index
        .iter()
        .filter(|(k, m)| {
            (k.starts_with("p:") || k.starts_with("h:") || k.starts_with("d:"))
                && m.iter().map(|&i| names[i].as_str()).collect::<std::collections::HashSet<_>>().len() >= BOILERPLATE_NAMES
        })
        .map(|(k, _)| k.as_str())
        .collect();
    let distinct: Vec<ContentSummary> = summaries
        .iter()
        .map(|s| {
            let mut d = s.clone();
            d.files.retain(|rel, f| {
                let by_path = format!("p:{rel}:{}", f.size);
                let by_hash = f.blake3.as_ref().map(|h| format!("h:{}", hex(h)));
                !boilerplate.contains(by_path.as_str()) && by_hash.is_none_or(|h| !boilerplate.contains(h.as_str()))
            });
            d
        })
        .collect();
    // Candidates as identity sees them: without boilerplate declared names (`vite_react_shadcn_ts`).
    let ident: Vec<ProjectCandidate> = cands
        .iter()
        .map(|c| {
            let mut c = c.clone();
            c.declared_names.retain(|d| !boilerplate.contains(format!("d:{}", d.to_lowercase()).as_str()));
            c
        })
        .collect();
    let mut pairs: std::collections::BTreeSet<(usize, usize)> = std::collections::BTreeSet::new();
    for (_, members) in index.iter().filter(|(k, m)| m.len() > 1 && m.len() <= 50 && !boilerplate.contains(k.as_str())) {
        for (x, &i) in members.iter().enumerate() {
            for &j in &members[x + 1..] {
                pairs.insert((i.min(j), i.max(j)));
            }
        }
    }
    let answered: Vec<usize> = keep
        .iter()
        .copied()
        .filter(|&i| answers.iter().any(|d| d.kind == DecisionKind::SameAs && (fingerprints_match(&d.fingerprint_a, &fps[i]) || d.fingerprint_b.as_deref().is_some_and(|b| fingerprints_match(b, &fps[i])))))
        .collect();
    for (x, &i) in answered.iter().enumerate() {
        for &j in &answered[x + 1..] {
            pairs.insert((i.min(j), i.max(j)));
        }
    }
    let mut parent: Vec<usize> = (0..cands.len()).collect();
    let mut identities: HashMap<(usize, usize), Identity> = HashMap::new();
    // Confirmed links first, so a "possibly the same" never decides a group.
    let mut found: Vec<((usize, usize), Identity)> = pairs
        .into_iter()
        .filter_map(|(i, j)| {
            identity_of(&ident[i], &ident[j], &distinct[i], &distinct[j], &fps[i], &fps[j], &answers).map(|id| ((i, j), id))
        })
        .collect();
    found.sort_by_key(|(p, id)| (!id.confirmed, *p));
    let mut sets: HashMap<usize, Vec<usize>> = keep.iter().map(|&i| (i, vec![i])).collect();
    for ((i, j), id) in found {
        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
        if ri != rj {
            // Never join through a third copy what the owner said are different projects.
            let ruled_out = sets[&ri]
                .iter()
                .any(|&x| sets[&rj].iter().any(|&y| answered_same(&fps[x], &fps[y], &answers) == Some(false)));
            // "Possibly the same" only ever pairs two folders that have nothing else: it never
            // pulls a third folder into a project on name alone.
            if ruled_out || (!id.confirmed && (sets[&ri].len() > 1 || sets[&rj].len() > 1)) {
                continue;
            }
            parent[rj] = ri;
            let moved = sets.remove(&rj).unwrap_or_default();
            sets.entry(ri).or_default().extend(moved);
        }
        identities.insert((i, j), id);
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &i in &keep {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }

    let mut collections: Vec<Collection> = Vec::new();
    for members in groups.into_values() {
        let collection_id = new_id();
        let mut locations: Vec<Location> = members
            .iter()
            .map(|&i| {
                let c = &cands[i];
                Location {
                    id: new_id(),
                    collection_id: collection_id.clone(),
                    volume_id: c.volume_id.clone(),
                    root_path: c.root.clone(),
                    relationship: Relationship::Unknown,
                    files: c.files,
                    bytes: c.bytes,
                    newest_mtime: c.newest_mtime,
                    shared_files: 0,
                    compared_by: String::new(),
                    is_repo: c.is_repo,
                    online: online.get(&c.volume_id).copied().unwrap_or(false),
                    fingerprint: fps[i].clone(),
                }
            })
            .collect();
        let h = choose_home(&locations);
        let home = members[h];
        let mut evidence = cands[home].evidence.clone();
        for (k, &m) in members.iter().enumerate() {
            if k == h {
                locations[k].relationship = Relationship::Home;
                locations[k].shared_files = summaries[m].len();
                continue;
            }
            let (mut rel, shared, by) = relate(&summaries[home], &summaries[m]);
            let pair = identities.get(&(home.min(m), home.max(m)));
            if version_series(cands[home].name(), cands[m].name()) && !matches!(rel, Relationship::Identical | Relationship::OlderSubset) {
                rel = Relationship::Version;
            } else if pair.is_none_or(|p| !p.confirmed) && shared == 0 {
                // Only the names agree: "Possibly the same project?"
                rel = Relationship::Unknown;
            }
            locations[k].relationship = rel;
            locations[k].shared_files = shared;
            locations[k].compared_by = by;
            let why = pair.map_or_else(|| "linked through another copy".to_string(), |p| p.evidence.clone());
            evidence.push(format!("also at {}: {why}", cands[m].root));
        }
        let any = |kind: DecisionKind| members.iter().find_map(|&i| answer_for(&answers, kind, &fps[i]));
        let state = if any(DecisionKind::LeaveAsIs).is_some() {
            "left_as_is"
        } else if any(DecisionKind::IsProject).is_some_and(|d| d.value == "yes") {
            "confirmed"
        } else {
            "candidate"
        };
        let confidence = members.iter().map(|&i| cands[i].confidence).fold(PROJECT_THRESHOLD, f64::max);
        collections.push(Collection {
            id: collection_id,
            kind: CollectionKind::Project,
            name: any(DecisionKind::Name).map_or_else(|| cands[home].name().to_string(), |d| d.value.clone()),
            project_kind: Some(cands[home].project_kind),
            confidence,
            state: state.into(),
            evidence,
            locations,
            sub_projects: cands[home].sub_projects.clone(),
        });
    }
    collections.sort_by_key(|c| (std::cmp::Reverse(c.locations.len().min(2)), c.name.to_lowercase()));
    save_collections(store.conn(), &collections)?;
    store.set_setting(RECOGNISED_AT, "context", &now_ms().to_string())?;
    Ok(RecogniseReport {
        projects: collections.len(),
        multi_location: collections.iter().filter(|c| c.locations.len() > 1).count(),
        took_ms: t.elapsed().as_millis() as u64,
    })
}
