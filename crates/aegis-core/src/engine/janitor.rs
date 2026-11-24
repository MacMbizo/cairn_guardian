//! Downloads janitor (spec §6.5): finds abandoned partial downloads, repeat downloads and
//! installers that have done their job, and *proposes* quarantining them. Nothing is removed
//! without the owner's approval; quarantine keeps everything for 30 days.

use super::{Finding, FindingAction, Level, Watch, bytes_text};
use crate::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use crate::db::SqliteStore;
use crate::db::actions::propose_batch;
use crate::db::files::normalize_path;
use crate::error::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use ts_rs::TS;

const DAY_MS: i64 = 86_400_000;
const PARTIAL_EXTS: [&str; 4] = ["crdownload", "part", "partial", "download"];
const INSTALLER_EXTS: [&str; 3] = ["exe", "msi", "msix"];
pub const PARTIAL_AGE_MS: i64 = DAY_MS;
pub const INSTALLER_AGE_DAYS: i64 = 14;
const AGENT: &str = "janitor";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum JanitorKind {
    Partial,
    Repeat { of: String },
    Installer {
        already_installed: bool,
        #[ts(type = "number")]
        age_days: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JanitorItem {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: i64,
    pub kind: JanitorKind,
}

struct Entry {
    path: PathBuf,
    display: String,
    size: i64,
    mtime: i64,
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Every regular file under `dir`, without following links.
fn walk(dir: &Path, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(&e.path(), out);
        } else if meta.is_file() {
            let path = e.path();
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            out.push(Entry {
                display: normalize_path(&canon, false).0,
                path,
                size: meta.len() as i64,
                mtime: mtime_ms(&meta),
            });
        }
    }
}

fn ext(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// `"invoice (2)"` → `Some("invoice")`.
fn repeat_base(stem: &str) -> Option<&str> {
    let inner = stem.strip_suffix(')')?;
    let open = inner.rfind(" (")?;
    let digits = &inner[open + 2..];
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then(|| &inner[..open])
}

fn blake3_of(p: &Path) -> Option<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    let mut f = std::fs::File::open(p).ok()?;
    std::io::copy(&mut f, &mut h).ok()?;
    Some(*h.finalize().as_bytes())
}

/// Paths already in a proposed or approved batch (of any agent).
fn in_open_batches(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT lower(a.from_path) FROM actions a JOIN action_batches b ON b.id = a.batch_id
         WHERE a.status IN ('proposed', 'approved') AND b.status IN ('draft', 'approved')",
    )?;
    Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Files the owner already said no to: janitor actions they rejected, or quarantined files
/// they restored. The janitor never proposes these again.
fn declined(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT lower(a.from_path) FROM actions a WHERE a.agent = ?1 AND a.status = 'rejected'
         UNION
         SELECT lower(q.original_path) FROM quarantine_entries q JOIN actions a ON a.id = q.action_id
         WHERE a.agent = ?1 AND q.restored_at IS NOT NULL",
    )?;
    Ok(stmt.query_map([AGENT], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Finds janitor candidates in `downloads` (subfolders included, links not followed).
pub fn scan_downloads(conn: &Connection, downloads: &Path, installed: &[String], now: i64) -> Result<Vec<JanitorItem>> {
    let mut entries = Vec::new();
    walk(downloads, &mut entries);
    let busy = in_open_batches(conn)?;
    let no = declined(conn)?;
    entries.retain(|e| {
        let key = e.display.to_lowercase();
        !busy.contains(&key) && !no.contains(&key)
    });

    let mut items = Vec::new();
    // Partial downloads.
    for e in &entries {
        if PARTIAL_EXTS.contains(&ext(&e.path).as_str()) && now - e.mtime > PARTIAL_AGE_MS {
            items.push(JanitorItem { path: e.display.clone(), bytes: e.size, kind: JanitorKind::Partial });
        }
    }
    // Repeat downloads: "name (n).ext" next to "name.ext" (or another copy) with identical bytes.
    let mut groups: BTreeMap<(PathBuf, String, String), Vec<&Entry>> = BTreeMap::new();
    for e in &entries {
        let stem = e.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let base = repeat_base(stem).unwrap_or(stem).to_string();
        let parent = e.path.parent().map(Path::to_path_buf).unwrap_or_default();
        groups.entry((parent, base.to_lowercase(), ext(&e.path))).or_default().push(e);
    }
    for members in groups.values() {
        let has_copy = members.iter().any(|m| {
            repeat_base(m.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default()).is_some()
        });
        if members.len() < 2 || !has_copy {
            continue;
        }
        let mut by_hash: BTreeMap<(i64, [u8; 32]), Vec<&Entry>> = BTreeMap::new();
        for m in members {
            if let Some(h) = blake3_of(&m.path) {
                by_hash.entry((m.size, h)).or_default().push(m);
            }
        }
        for mut same in by_hash.into_values().filter(|v| v.len() > 1) {
            same.sort_by_key(|m| m.mtime);
            let keeper = same[0].path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            for m in &same[1..] {
                items.push(JanitorItem {
                    path: m.display.clone(),
                    bytes: m.size,
                    kind: JanitorKind::Repeat { of: keeper.clone() },
                });
            }
        }
    }
    // Installers that have sat for a while: only loose files in Downloads itself. An .exe
    // inside a folder is usually part of an unzipped or portable app, not an installer.
    for e in entries.iter().filter(|e| e.path.parent() == Some(downloads)) {
        let age_days = (now - e.mtime) / DAY_MS;
        if INSTALLER_EXTS.contains(&ext(&e.path).as_str()) && age_days >= INSTALLER_AGE_DAYS {
            let stem = e.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            items.push(JanitorItem {
                path: e.display.clone(),
                bytes: e.size,
                kind: JanitorKind::Installer { already_installed: installer_matches(stem, installed), age_days },
            });
        }
    }
    // One item per file: a repeat download of an old installer matches two rules. Items were
    // pushed partial → repeat → installer, so the first (most specific) reason wins.
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert(i.path.to_lowercase()));
    Ok(items)
}

const NOISE_TOKENS: [&str; 13] = [
    "setup", "installer", "install", "x64", "x86", "amd64", "arm64", "win", "win64", "win32", "full", "user", "64",
];

/// Lowercase words, split on punctuation and camelCase boundaries, without installer noise.
fn tokens(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .into_iter()
        .map(|w| w.to_lowercase())
        .filter(|w| !NOISE_TOKENS.contains(&w.as_str()) && !w.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

/// True when an installer's file name names a program that is already installed. Heuristic:
/// the first meaningful word must match the program's first word (and the second, when both
/// have one). When unsure this says no, and the janitor only says "not opened in N days".
pub fn installer_matches(file_stem: &str, installed: &[String]) -> bool {
    let t = tokens(file_stem);
    let Some(first) = t.first() else { return false };
    installed.iter().any(|name| {
        let n = tokens(name);
        n.first() == Some(first)
            && match (t.get(1), n.get(1)) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    })
}

/// Display names of installed programs (Uninstall registry keys, 64- and 32-bit, machine and user).
#[cfg(windows)]
pub fn installed_programs() -> Vec<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, RRF_RT_REG_SZ,
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let path = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall");
    let value = wide("DisplayName");
    let mut out = Vec::new();
    for (root, view) in [
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, 0),
    ] {
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated strings and out-pointers; the key is closed below.
        if unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) } != 0 {
            continue;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(
                    key,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc != 0 {
                break;
            }
            i += 1;
            let mut sub = name[..len as usize].to_vec();
            sub.push(0);
            let mut buf = [0u16; 512];
            let mut size = (buf.len() * 2) as u32;
            let rc = unsafe {
                RegGetValueW(
                    key,
                    sub.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if rc == 0 {
                let n = (size as usize / 2).saturating_sub(1);
                let s = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
                if !s.trim().is_empty() {
                    out.push(s);
                }
            }
        }
        unsafe { RegCloseKey(key) };
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(not(windows))]
pub fn installed_programs() -> Vec<String> {
    Vec::new()
}

/// The janitor's open (unanswered) batch, if any.
pub fn open_janitor_batch(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT b.id FROM action_batches b
             WHERE b.status IN ('draft', 'approved')
               AND EXISTS (SELECT 1 FROM actions a WHERE a.batch_id = b.id AND a.agent = ?1 AND a.status IN ('proposed', 'approved'))
             ORDER BY b.created_at DESC LIMIT 1",
            [AGENT],
            |r| r.get(0),
        )
        .optional()?)
}

fn reason(kind: &JanitorKind) -> String {
    match kind {
        JanitorKind::Partial => "Unfinished download, untouched for a day".into(),
        JanitorKind::Repeat { of } => format!("Same file as {of}"),
        JanitorKind::Installer { already_installed: true, .. } => "Installer for a program you already have".into(),
        JanitorKind::Installer { age_days, .. } => format!("Installer not opened in {age_days} days"),
    }
}

/// Proposes quarantining `items` as one batch. `None` when there is nothing to propose or the
/// janitor already has a batch waiting for the owner (one at a time).
pub fn propose(store: &mut SqliteStore, items: &[JanitorItem]) -> Result<Option<String>> {
    if items.is_empty() || open_janitor_batch(store.conn())?.is_some() {
        return Ok(None);
    }
    let lookup = |store: &SqliteStore, path: &str| -> Result<Option<String>> {
        Ok(store
            .conn()
            .query_row(
                "SELECT id FROM files WHERE lower(path) = lower(?1) AND deleted_at IS NULL LIMIT 1",
                [path],
                |r| r.get(0),
            )
            .optional()?)
    };
    // A file may have only one proposed/approved action at a time (any feature, any batch).
    let has_active = |store: &SqliteStore, file_id: &str| -> Result<bool> {
        Ok(store.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM actions WHERE file_id = ?1 AND status IN ('proposed', 'approved'))",
            [file_id],
            |r| r.get(0),
        )?)
    };
    let mut taken = HashSet::new();
    let mut actions = Vec::new();
    for it in items {
        let mut file_id = lookup(store, &it.path)?;
        if file_id.is_none() {
            // A download the live index hasn't reached yet: index it (metadata only) first.
            let vol = store.ensure_volume_for_path(Path::new(&it.path))?;
            crate::scanner::index_paths(store, &vol.id, &[PathBuf::from(&it.path)])?;
            file_id = lookup(store, &it.path)?;
        }
        let Some(file_id) = file_id else { continue };
        if !taken.insert(file_id.clone()) || has_active(store, &file_id)? {
            continue;
        }
        let file_id = Some(file_id);
        actions.push(ProposedActionInput {
            agent: Some(AGENT.into()),
            action_type: ActionType::Quarantine,
            target_kind: TargetKind::File,
            file_id,
            atomic_unit_id: None,
            from_path: it.path.clone(),
            to_path: None,
            reason: reason(&it.kind),
            confidence: 0.9,
            evidence_json: serde_json::to_string(&it.kind)?,
        });
    }
    if actions.is_empty() {
        return Ok(None);
    }
    let batch = propose_batch(store, &RefilePlan { origin: "janitor".into(), actions })?;
    Ok(Some(batch))
}

/// Bytes still present across the open janitor batch.
fn open_batch_bytes(conn: &Connection, batch: &str) -> Result<i64> {
    let mut stmt = conn.prepare("SELECT from_path FROM actions WHERE batch_id = ?1 AND status IN ('proposed', 'approved')")?;
    let paths: Vec<String> = stmt.query_map([batch], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len() as i64).sum())
}

pub struct JanitorWatch {
    pub downloads: PathBuf,
    pub installed: Box<dyn Fn() -> Vec<String> + Send>,
}

impl Watch for JanitorWatch {
    fn name(&self) -> &'static str {
        "janitor"
    }
    fn every_ms(&self) -> i64 {
        DAY_MS
    }
    fn needs_idle(&self) -> bool {
        true
    }
    fn kinds(&self) -> &'static [&'static str] {
        &["janitor"]
    }
    fn run(&mut self, store: &mut SqliteStore, now: i64) -> Result<Vec<Finding>> {
        if !self.downloads.is_dir() {
            return Ok(vec![]);
        }
        let installed = (self.installed)();
        let items = scan_downloads(store.conn(), &self.downloads, &installed, now)?;
        let batch = match propose(store, &items)? {
            Some(b) => Some(b),
            None => open_janitor_batch(store.conn())?,
        };
        let Some(batch) = batch else { return Ok(vec![]) };
        let bytes = open_batch_bytes(store.conn(), &batch)?;
        if bytes == 0 {
            return Ok(vec![]);
        }
        Ok(vec![Finding {
            kind: "janitor".into(),
            subject: "downloads".into(),
            level: Level::Note,
            title: format!("{} of old installers and repeat downloads", bytes_text(bytes)),
            detail: "Ready to review. Nothing is removed until you approve, and quarantine keeps it 30 days.".into(),
            bytes: Some(bytes),
            action: FindingAction::Batch { batch_id: batch },
        }])
    }
}
