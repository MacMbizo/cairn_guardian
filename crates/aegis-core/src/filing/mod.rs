//! Auto-filing (spec 2026-11-21-auto-filing-design): reads what files contain, works out what
//! they are, proposes a sensible folder structure and names per drive, moves files in batches
//! the owner approves, and learns rules that file new arrivals by themselves.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

static LOCAL_OFFSET: std::sync::OnceLock<fn(i64) -> i32> = std::sync::OnceLock::new();

/// Sets how far local time is ahead of UTC (in seconds) at a given UTC instant (ms). The desktop
/// host sets it once at start-up from the system time zone; until then local time is UTC.
pub fn set_local_offset(offset_secs_at: fn(i64) -> i32) {
    let _ = LOCAL_OFFSET.set(offset_secs_at);
}

/// A UTC timestamp (ms) moved to local wall-clock time (ms), for dates and times in names.
pub fn local_ms(utc_ms: i64) -> i64 {
    utc_ms + LOCAL_OFFSET.get().map_or(0, |f| i64::from(f(utc_ms)) * 1000)
}

/// What sort of thing a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Kind {
    Document,
    Photo,
    Screenshot,
    Video,
    Music,
    Audio,
    Code,
    Installer,
    Archive,
    Font,
    System,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Photo => "photo",
            Self::Screenshot => "screenshot",
            Self::Video => "video",
            Self::Music => "music",
            Self::Audio => "audio",
            Self::Code => "code",
            Self::Installer => "installer",
            Self::Archive => "archive",
            Self::Font => "font",
            Self::System => "system",
            Self::Other => "other",
        }
    }

    pub fn from_db(s: &str) -> Kind {
        match s {
            "document" => Self::Document,
            "photo" => Self::Photo,
            "screenshot" => Self::Screenshot,
            "video" => Self::Video,
            "music" => Self::Music,
            "audio" => Self::Audio,
            "code" => Self::Code,
            "installer" => Self::Installer,
            "archive" => Self::Archive,
            "font" => Self::Font,
            "system" => Self::System,
            _ => Self::Other,
        }
    }
}

/// What Aegis read and concluded about one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileMeta {
    pub file_id: String,
    pub kind: Kind,
    pub category: Option<String>,
    pub doc_type: Option<String>,
    pub who: Option<String>,
    /// `YYYY-MM-DD`.
    pub doc_date: Option<String>,
    /// Where `doc_date` came from: only content, name and photo dates are trusted for naming.
    pub date_source: classify::DateSource,
    pub title: Option<String>,
    #[ts(type = "number | null")]
    pub exif_taken_at: Option<i64>,
    pub camera: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub confidence: f64,
    pub evidence: Vec<String>,
    #[ts(type = "number | null")]
    pub read_at: Option<i64>,
    pub read_error: Option<String>,
}

impl FileMeta {
    /// Empty metadata for a file of `kind`.
    pub fn new(file_id: &str, kind: Kind) -> Self {
        Self {
            file_id: file_id.to_string(),
            kind,
            category: None,
            doc_type: None,
            who: None,
            doc_date: None,
            date_source: classify::DateSource::None,
            title: None,
            exif_taken_at: None,
            camera: None,
            artist: None,
            album: None,
            confidence: 0.0,
            evidence: Vec::new(),
            read_at: None,
            read_error: None,
        }
    }
}

pub fn save_meta(conn: &Connection, m: &FileMeta) -> Result<()> {
    conn.execute(
        "INSERT INTO file_meta (file_id, kind, category, doc_type, who, doc_date, title, exif_taken_at, camera,
                                artist, album, confidence, evidence_json, read_at, read_error, date_source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(file_id) DO UPDATE SET
            kind = excluded.kind, category = excluded.category, doc_type = excluded.doc_type,
            who = excluded.who, doc_date = excluded.doc_date, title = excluded.title,
            exif_taken_at = excluded.exif_taken_at, camera = excluded.camera, artist = excluded.artist,
            album = excluded.album, confidence = excluded.confidence, evidence_json = excluded.evidence_json,
            read_at = excluded.read_at, read_error = excluded.read_error, date_source = excluded.date_source",
        params![
            m.file_id,
            m.kind.as_str(),
            m.category,
            m.doc_type,
            m.who,
            m.doc_date,
            m.title,
            m.exif_taken_at,
            m.camera,
            m.artist,
            m.album,
            m.confidence,
            serde_json::to_string(&m.evidence)?,
            m.read_at,
            m.read_error,
            m.date_source.as_str()
        ],
    )?;
    Ok(())
}

pub fn load_meta(conn: &Connection, file_id: &str) -> Result<Option<FileMeta>> {
    let row = conn
        .query_row(
            "SELECT kind, category, doc_type, who, doc_date, title, exif_taken_at, camera, artist, album,
                    confidence, evidence_json, read_at, read_error, date_source
             FROM file_meta WHERE file_id = ?1",
            [file_id],
            |r| {
                let evidence: String = r.get(11)?;
                Ok(FileMeta {
                    file_id: file_id.to_string(),
                    kind: Kind::from_db(&r.get::<_, String>(0)?),
                    category: r.get(1)?,
                    doc_type: r.get(2)?,
                    who: r.get(3)?,
                    doc_date: r.get(4)?,
                    date_source: classify::DateSource::from_db(r.get::<_, Option<String>>(14)?.as_deref()),
                    title: r.get(5)?,
                    exif_taken_at: r.get(6)?,
                    camera: r.get(7)?,
                    artist: r.get(8)?,
                    album: r.get(9)?,
                    confidence: r.get(10)?,
                    evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                    read_at: r.get(12)?,
                    read_error: r.get(13)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Where filing applies: the PC's own folders, or one drive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Scope {
    Pc,
    Volume { volume_id: String },
}

impl Scope {
    /// Stable key used in tables: `"pc"` or the volume id.
    pub fn key(&self) -> String {
        match self {
            Self::Pc => "pc".into(),
            Self::Volume { volume_id } => volume_id.clone(),
        }
    }
}

/// Which structure the owner chose for a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BlueprintChoice {
    /// The owner's own folders, cleaned up.
    Tidy,
    /// The standard structure.
    Fresh,
}

impl BlueprintChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tidy => "tidy",
            Self::Fresh => "fresh",
        }
    }
}

fn counts_key(scope: &Scope) -> String {
    format!("filing.counts:{}", scope.key())
}

/// Cached `(files still to move, files to sort)` for a scope, so the Filing screen doesn't
/// re-plan a whole drive on every visit.
pub fn cached_counts(conn: &Connection, scope: &Scope) -> Result<Option<(i64, i64)>> {
    let raw: Option<String> = conn
        .query_row("SELECT value_json FROM settings WHERE key = ?1 AND scope = 'global'", [counts_key(scope)], |r| r.get(0))
        .optional()?;
    Ok(raw.and_then(|j| serde_json::from_str(&j).ok()))
}

pub fn set_counts(conn: &Connection, scope: &Scope, remaining: i64, to_sort: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, scope, value_json, updated_at) VALUES (?1, 'global', ?2, ?3)
         ON CONFLICT(key, scope) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        params![counts_key(scope), serde_json::to_string(&(remaining.max(0), to_sort.max(0)))?, crate::now_ms()],
    )?;
    Ok(())
}

/// Forgets a scope's cached counts, so the next visit counts afresh (after an undo, a skip,
/// new arrivals or a new choice).
pub fn clear_counts(conn: &Connection, scope: &Scope) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1 AND scope = 'global'", [counts_key(scope)])?;
    Ok(())
}

pub fn set_blueprint(conn: &Connection, scope: &Scope, choice: BlueprintChoice, now: i64) -> Result<()> {
    // A new choice means new counts.
    clear_counts(conn, scope)?;
    conn.execute(
        "INSERT INTO filing_blueprints (scope_key, choice, chosen_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(scope_key) DO UPDATE SET choice = excluded.choice, chosen_at = excluded.chosen_at",
        params![scope.key(), choice.as_str(), now],
    )?;
    Ok(())
}

pub fn blueprint_of(conn: &Connection, scope: &Scope) -> Result<Option<BlueprintChoice>> {
    let c: Option<String> = conn
        .query_row("SELECT choice FROM filing_blueprints WHERE scope_key = ?1", [scope.key()], |r| r.get(0))
        .optional()?;
    Ok(c.map(|c| if c == "tidy" { BlueprintChoice::Tidy } else { BlueprintChoice::Fresh }))
}

pub mod extract;

pub mod classify;
pub mod reader;
pub mod orgs;
pub mod naming;
pub mod blueprint;
pub mod rules;
pub mod batches;
pub mod watch;

/// One row on the Filing screen: the PC or a drive, and how far filing has got.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScopeStatus {
    pub scope: Scope,
    pub label: String,
    pub online: bool,
    /// Share of files Aegis has read or classified, 0–1.
    pub read_pct: f64,
    pub blueprint: Option<BlueprintChoice>,
    pub open_batch: Option<String>,
    #[ts(type = "number")]
    pub filed: i64,
    /// Files the chosen structure would still move (0 without a choice).
    #[ts(type = "number")]
    pub remaining: i64,
    #[ts(type = "number")]
    pub to_sort: i64,
    /// False when the drive is unplugged and Aegis has no counts for it yet.
    pub counts_known: bool,
}

/// The PC and every catalogued general drive (never the Backup drive), with progress.
pub fn scope_statuses(store: &mut crate::db::SqliteStore) -> Result<Vec<ScopeStatus>> {
    let mut scopes: Vec<(Scope, String, bool)> = vec![(Scope::Pc, "This PC".into(), true)];
    {
        let mut stmt = store.conn().prepare(
            "SELECT id, COALESCE(label, mount_point), status = 'online' FROM volumes
             WHERE passport_id IS NOT NULL AND last_catalog_complete_at IS NOT NULL AND COALESCE(role, '') != 'backup'
             ORDER BY 2",
        )?;
        for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))? {
            let (id, label, online) = r?;
            scopes.push((Scope::Volume { volume_id: id }, label, online));
        }
    }
    let mut out = Vec::new();
    for (scope, label, online) in scopes {
        let roots: Vec<String> = blueprint::scope_roots(store, &scope)?.into_iter().map(|(p, _)| p).collect();
        // Read coverage counted by SQL (one aggregate per root), not by walking every row.
        let (mut files, mut read) = (0i64, 0i64);
        for root in &roots {
            let (f, r): (i64, i64) = store.conn().query_row(
                "SELECT COUNT(*), COUNT(m.file_id) FROM files f LEFT JOIN file_meta m ON m.file_id = f.id
                 WHERE f.deleted_at IS NULL AND lower(substr(f.path, 1, ?2)) = lower(?1)",
                params![root, root.chars().count() as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            files += f;
            read += r;
        }
        let filed: i64 = store.conn().query_row(
            "SELECT COUNT(*) FROM file_history h JOIN action_batches b ON b.id = h.batch_id
             WHERE b.origin IN (?1, ?2)",
            [format!("filing:{}", scope.key()), format!("filing-auto:{}", scope.key())],
            |r| r.get(0),
        )?;
        let choice = blueprint_of(store.conn(), &scope)?;
        let (mut remaining, mut to_sort, mut counts_known) = (0, 0, true);
        if let Some(c) = choice {
            if let Some((r, t)) = cached_counts(store.conn(), &scope)? {
                (remaining, to_sort) = (r, t);
            } else if online {
                (remaining, to_sort) = batches::recount(store, &scope, c)?;
            } else {
                counts_known = false;
            }
        }
        out.push(ScopeStatus {
            open_batch: batches::open_filing_batch(store.conn(), &scope)?,
            scope,
            label,
            online,
            read_pct: if files == 0 { 1.0 } else { read as f64 / files as f64 },
            blueprint: choice,
            filed,
            remaining,
            to_sort,
            counts_known,
        });
    }
    Ok(out)
}
