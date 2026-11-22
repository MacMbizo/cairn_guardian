//! Identity and relationships (spec §6, §7.3), shared by every module: are two locations the
//! same thing, and if so how does one copy relate to the other (identical, an older subset,
//! changed on both sides, different material, a version)?
//!
//! Comparisons run over files that aren't build output and aren't trivial (under 1 KB, or
//! generic like `LICENSE`). Files match by BLAKE3 when both sides have one, else by relative
//! path and size.

use super::profile::{live_under_root_sql, name_words};
use super::projects::ProjectCandidate;
use super::{Decision, DecisionKind, Location, Relationship};
use crate::db::SqliteStore;
use crate::error::Result;
use rusqlite::params;
use std::collections::{BTreeMap, HashSet};

const SEP: char = std::path::MAIN_SEPARATOR;

/// Files under this size are trivial and never count toward a comparison.
pub const TRIVIAL_BYTES: i64 = 1024;

/// Names too generic to say two folders share anything.
const GENERIC_NAMES: [&str; 12] = [
    "license",
    "license.md",
    "license.txt",
    ".gitignore",
    ".gitattributes",
    ".editorconfig",
    ".prettierrc",
    "favicon.ico",
    "desktop.ini",
    "thumbs.db",
    ".ds_store",
    "package-lock.json",
];

/// Name words that say nothing about which project it is.
const NOISE_WORDS: [&str; 7] = ["app", "main", "backup", "copy", "old", "final", "new"];

/// A project name reduced to what identifies it: lowercase, without separators, `app`, `main`,
/// `backup`, `copy`, `old`, `final`, `new`, bracketed notes and trailing versions.
/// `wellness_app` → `wellness`, `Inv2` → `inv`, `set_files_v2(backup)` → `setfiles`.
pub fn normalized_name(name: &str) -> String {
    let mut n = name.to_string();
    while let (Some(a), Some(b)) = (n.find('('), n.find(')')) {
        if b < a {
            break;
        }
        n.replace_range(a..=b, " ");
    }
    let mut words: Vec<String> = name_words(&n).into_iter().filter(|w| !NOISE_WORDS.contains(&w.as_str())).collect();
    // Trailing versions: `v2`, `2`, `1.1`, and digits glued to the last word (`inv2`).
    while let Some(last) = words.last() {
        let is_version = last.chars().all(|c| c.is_ascii_digit())
            || (last.starts_with('v') && last.len() > 1 && last[1..].chars().all(|c| c.is_ascii_digit()));
        if is_version && words.len() > 1 {
            words.pop();
        } else {
            break;
        }
    }
    if let Some(last) = words.last_mut() {
        let trimmed = last.trim_end_matches(|c: char| c.is_ascii_digit());
        if !trimmed.is_empty() {
            *last = trimmed.to_string();
        }
    }
    words.concat()
}

/// Whether two folder names are versions in one series (`Inv`, `Inv2`, `Inv3`).
pub fn version_series(a: &str, b: &str) -> bool {
    let base = |s: &str| {
        let l = s.to_lowercase();
        let t = l.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        let t = ["_v", "-v", " v"].iter().find_map(|v| t.strip_suffix(v)).unwrap_or(t);
        t.trim_end_matches(['_', '-', ' ']).to_string()
    };
    let (ba, bb) = (base(a), base(b));
    !a.eq_ignore_ascii_case(b) && ba == bb && ba.chars().count() >= 3
}

/// One file of a location, as compared.
#[derive(Debug, Clone, PartialEq)]
pub struct FileKey {
    pub size: i64,
    pub mtime: i64,
    pub blake3: Option<Vec<u8>>,
}

/// What a location holds, for comparison: relative path (lowercase) → file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContentSummary {
    pub files: BTreeMap<String, FileKey>,
    pub newest_mtime: i64,
}

impl ContentSummary {
    pub fn len(&self) -> i64 {
        self.files.len() as i64
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn hashes(&self) -> HashSet<&[u8]> {
        self.files.values().filter_map(|f| f.blake3.as_deref()).collect()
    }
}

/// The non-trivial, non-build files of the location `root` on `volume_id`.
pub fn content_summary(store: &SqliteStore, volume_id: &str, root: &str) -> Result<ContentSummary> {
    let root = root.trim_end_matches(SEP);
    let prefix = format!("{}{SEP}", root.to_lowercase());
    let plen = prefix.chars().count() as i64;
    let mut stmt = store.conn().prepare(&format!(
        "SELECT path, name, size, mtime, blake3 FROM files
         WHERE {} AND volume_id = ?3 AND size >= ?4",
        live_under_root_sql()
    ))?;
    let rows = stmt.query_map(params![prefix, plen, volume_id, TRIVIAL_BYTES], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, Option<Vec<u8>>>(4)?,
        ))
    })?;
    let mut out = ContentSummary::default();
    for row in rows {
        let (path, name, size, mtime, blake3) = row?;
        if GENERIC_NAMES.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        let rel = path.chars().skip(prefix.chars().count()).collect::<String>().to_lowercase();
        out.newest_mtime = out.newest_mtime.max(mtime);
        out.files.insert(rel, FileKey { size, mtime, blake3 });
    }
    Ok(out)
}

/// Why two locations are taken to be one project. `confirmed` is false when only their names
/// agree: the card then asks "Possibly the same project?" rather than assuming it.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub evidence: String,
    pub confirmed: bool,
}

/// Files of `other` also in `home`, whether `other` holds newer versions of shared paths, and
/// whether most of the comparison was by content.
fn overlap(home: &ContentSummary, other: &ContentSummary) -> (i64, bool, bool) {
    let home_hashes = home.hashes();
    let (mut shared, mut hashed, mut newer) = (0i64, 0i64, false);
    for (rel, f) in &other.files {
        let same_path = home.files.get(rel);
        if let Some(h) = f.blake3.as_deref()
            && !home_hashes.is_empty()
        {
            hashed += 1;
            if home_hashes.contains(h) {
                shared += 1;
                continue;
            }
            if let Some(hf) = same_path
                && hf.blake3.is_some()
            {
                // Same path, different content: changed in one copy.
                if f.mtime > hf.mtime + 2_000 {
                    newer = true;
                }
                continue;
            }
        }
        if let Some(hf) = same_path {
            if hf.size == f.size {
                shared += 1;
            }
            if f.mtime > hf.mtime + 2_000 {
                newer = true;
            }
        }
    }
    (shared, newer, hashed * 2 >= other.len().max(1))
}

/// How `other` relates to `home` (spec §6): the relationship, how many of `other`'s files are
/// also in `home`, and how they were compared (`content` or `name and size`).
pub fn relate(home: &ContentSummary, other: &ContentSummary) -> (Relationship, i64, String) {
    let (shared, newer, by_content) = overlap(home, other);
    let compared_by = if by_content { "content" } else { "name and size" }.to_string();
    let (a, h) = (other.len(), home.len());
    if a == 0 || h == 0 {
        return (Relationship::Unknown, shared, compared_by);
    }
    let smaller = a.min(h);
    let rel = if shared * 100 >= a * 98 && shared * 100 >= h * 98 {
        Relationship::Identical
    } else if shared * 100 >= a * 98 && !newer && home.newest_mtime >= other.newest_mtime {
        Relationship::OlderSubset
    } else if shared * 10 < smaller {
        Relationship::DifferentMaterial
    } else {
        Relationship::Diverged
    };
    (rel, shared, compared_by)
}

/// A location's stable identity for the owner's answers (spec §12): its normalized name, the
/// names its manifests declare, and short keys of its 20 biggest files. Survives a drive letter
/// change, and a rename as long as the contents stay.
pub fn fingerprint(name: &str, declared: &[String], summary: &ContentSummary) -> String {
    let mut biggest: Vec<(&String, &FileKey)> = summary.files.iter().collect();
    biggest.sort_by(|a, b| b.1.size.cmp(&a.1.size).then_with(|| a.0.cmp(b.0)));
    let mut keys: Vec<String> = biggest
        .into_iter()
        .take(20)
        .map(|(rel, f)| {
            let raw = match &f.blake3 {
                Some(h) => h.clone(),
                None => format!("{rel}:{}", f.size).into_bytes(),
            };
            blake3::hash(&raw).to_hex()[..8].to_string()
        })
        .collect();
    keys.sort();
    let mut names: Vec<String> = declared.iter().map(|d| d.to_lowercase()).collect();
    names.sort();
    names.dedup();
    format!("{}|{}|{}", normalized_name(name), names.join(","), keys.join(","))
}

/// Whether two fingerprints are the same location: half or more of their content keys agree,
/// or (when there is no content to go on) their names and manifests do.
pub fn fingerprints_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let parts = |s: &str| {
        let mut it = s.splitn(3, '|');
        (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
    };
    let ((na, ma, ka), (nb, mb, kb)) = (parts(a), parts(b));
    let ka: HashSet<&str> = ka.split(',').filter(|k| !k.is_empty()).collect();
    let kb: HashSet<&str> = kb.split(',').filter(|k| !k.is_empty()).collect();
    if !ka.is_empty() && !kb.is_empty() {
        let common = ka.intersection(&kb).count();
        return common * 2 >= ka.len().min(kb.len());
    }
    na == nb && ma == mb && !na.is_empty()
}

/// The owner's answer about this pair, if any: `Some(true)` for "same project", `Some(false)`
/// for "not the same project". The latest answer wins.
pub fn answered_same(fa: &str, fb: &str, decisions: &[Decision]) -> Option<bool> {
    decisions
        .iter()
        .filter(|d| matches!(d.kind, DecisionKind::SameAs | DecisionKind::NotSameAs))
        .filter(|d| {
            let Some(b) = d.fingerprint_b.as_deref() else { return false };
            (fingerprints_match(&d.fingerprint_a, fa) && fingerprints_match(b, fb))
                || (fingerprints_match(&d.fingerprint_a, fb) && fingerprints_match(b, fa))
        })
        .max_by_key(|d| d.at)
        .map(|d| d.kind == DecisionKind::SameAs)
}

/// Whether `a` and `b` are candidates for one project (spec §7.3), and why.
pub fn same_identity(
    a: &ProjectCandidate,
    b: &ProjectCandidate,
    sa: &ContentSummary,
    sb: &ContentSummary,
    decisions: &[Decision],
) -> Option<Identity> {
    let fa = fingerprint(a.name(), &a.declared_names, sa);
    let fb = fingerprint(b.name(), &b.declared_names, sb);
    match answered_same(&fa, &fb, decisions) {
        Some(true) => return Some(Identity { evidence: "you said these are the same project".into(), confirmed: true }),
        Some(false) => return None,
        None => {}
    }
    let declared_a: HashSet<String> = a.declared_names.iter().map(|n| n.to_lowercase()).collect();
    if let Some(n) = b.declared_names.iter().find(|n| declared_a.contains(&n.to_lowercase())) {
        return Some(Identity { evidence: format!("both call themselves \"{n}\""), confirmed: true });
    }
    let (shared, _, _) = overlap(sa, sb);
    let smaller = sa.len().min(sb.len());
    if shared >= 20 || (smaller > 0 && shared * 10 >= smaller * 3) {
        return Some(Identity { evidence: format!("{shared} files in common"), confirmed: true });
    }
    let (na, nb) = (normalized_name(a.name()), normalized_name(b.name()));
    if na.chars().count() >= 3 && na == nb {
        return Some(if shared > 0 {
            Identity { evidence: format!("same name and {shared} files in common"), confirmed: true }
        } else {
            Identity { evidence: "same name, nothing in common yet".into(), confirmed: false }
        });
    }
    None
}

/// The location a project's plan centres on (spec §8.1): a git repository first (the most
/// recently changed one if several), else the most recent activity, then the most files.
pub fn choose_home(locations: &[Location]) -> usize {
    let mut idx: Vec<usize> = (0..locations.len()).collect();
    idx.sort_by(|&i, &j| {
        let (a, b) = (&locations[i], &locations[j]);
        b.is_repo
            .cmp(&a.is_repo)
            .then(b.newest_mtime.cmp(&a.newest_mtime))
            .then(b.files.cmp(&a.files))
            .then(i.cmp(&j))
    });
    idx.first().copied().unwrap_or(0)
}
