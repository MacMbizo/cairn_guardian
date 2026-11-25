//! SQLite store, mandatory PRAGMAs, migrations, and settings/roots helpers (`B4 v2.3.1`).

pub mod actions;
pub mod audit;
pub mod crypto;
pub mod files;
pub mod search;
pub mod vec;

use crate::contracts::{IndexStats, RootConfig, VolumeInfo};
use crate::error::{AegisError, Result};
use crate::now_ms;
use crate::scanner::os_adapter::{disk_space_for_mount, inspect_volume_for_path};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;
use std::sync::Once;

static EXT_INIT: Once = Once::new();

/// Registers statically compiled extensions (`sqlite-vec`) on every SQLite/SQLCipher connection.
pub fn register_extensions() {
    EXT_INIT.call_once(|| {
        type ExtFn = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut core::ffi::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> core::ffi::c_int;
        // SAFETY: `sqlite_vec::sqlite3_vec_init` matches SQLite's auto-extension entrypoint signature
        // when compiled with `SQLITE_CORE`.
        unsafe {
            let fn_ptr: ExtFn =
                core::mem::transmute::<*const (), ExtFn>(sqlite_vec::sqlite3_vec_init as *const ());
            rusqlite::ffi::sqlite3_auto_extension(Some(fn_ptr));
        }
    });
}

/// Ordered schema migrations: (version, name, SQL). Each applies in its own transaction.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "0001_initial_b4_v2_3_1", include_str!("../../migrations/0001_initial_b4_v2_3_1.sql")),
    (2, "0002_drives_backup", include_str!("../../migrations/0002_drives_backup.sql")),
    (3, "0003_backup_version_source", include_str!("../../migrations/0003_backup_version_source.sql")),
    (4, "0004_engine", include_str!("../../migrations/0004_engine.sql")),
    (5, "0005_filing", include_str!("../../migrations/0005_filing.sql")),
    (6, "0006_context", include_str!("../../migrations/0006_context.sql")),
    (7, "0007_project_plans", include_str!("../../migrations/0007_project_plans.sql")),
    (8, "0008_sets", include_str!("../../migrations/0008_sets.sql")),
    (9, "0009_vault_and_vec_meta", include_str!("../../migrations/0009_vault_and_vec_meta.sql")),
    (10, "0010_forgotten_assets", include_str!("../../migrations/0010_forgotten_assets.sql")),
    (11, "0011_version_tracking", include_str!("../../migrations/0011_version_tracking.sql")),
    (12, "0012_knowledge_extraction", include_str!("../../migrations/0012_knowledge_extraction.sql")),
    (13, "0013_research_librarian", include_str!("../../migrations/0013_research_librarian.sql")),
    (14, "0014_cross_project", include_str!("../../migrations/0014_cross_project.sql")),
    (15, "0015_folderless_intents", include_str!("../../migrations/0015_folderless_intents.sql")),
    (16, "0016_self_healing", include_str!("../../migrations/0016_self_healing.sql")),
    (17, "0017_personal_data_governance", include_str!("../../migrations/0017_personal_data_governance.sql")),
    (18, "0018_cdo_agent", include_str!("../../migrations/0018_cdo_agent.sql")),
    (19, "0019_digital_estate", include_str!("../../migrations/0019_digital_estate.sql")),
];

/// Where the installed app keeps its database: `%LOCALAPPDATA%\com.aegis.guardian\aegis.db`.
pub fn live_database_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("com.aegis.guardian").join("aegis.db"))
}

/// For tools that write into whatever database they are given (the filing dry run): refuses the
/// owner's live database, and any database inside an app data folder (one with a WebView2
/// profile beside it), so they only ever run against a copy.
pub fn refuse_live_database(db: &Path, live: Option<&Path>) -> Result<()> {
    let canon = |p: &Path| std::fs::canonicalize(p).ok().map(|c| c.to_string_lossy().to_lowercase());
    let target = canon(db).ok_or_else(|| crate::error::AegisError::NotFound(db.display().to_string()))?;
    if live.and_then(canon).is_some_and(|l| l == target) {
        return Err(crate::error::AegisError::InvalidInput(
            "That is the live Cairn database. Copy it first and point the tool at the copy.".into(),
        ));
    }
    if db.parent().is_some_and(|d| d.join("EBWebView").is_dir()) {
        return Err(crate::error::AegisError::InvalidInput(
            "That database belongs to a Cairn data folder. Copy it somewhere else first.".into(),
        ));
    }
    Ok(())
}

/// Embedded SQLite store holding the single writer connection (`B4` §2).
pub struct SqliteStore {
    conn: Connection,
    in_memory: bool,
    encrypted: bool,
}

impl SqliteStore {
    /// Opens or creates an on-disk SQLite database with mandatory PRAGMAs and migrations.
    pub fn open(path: &Path) -> Result<Self> {
        register_extensions();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init_connection(conn, false, false)
    }

    /// Opens an encrypted SQLCipher connection WITHOUT applying PRAGMAs or migrations (for
    /// verification during migration or raw checks).
    pub(crate) fn open_encrypted_raw(path: &Path, key: &crypto::DbKey) -> Result<Connection> {
        register_extensions();
        let conn = Connection::open(path)?;
        let pragma = key.raw_key_pragma();
        conn.execute_batch(pragma.as_str())?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
            .map_err(|_| {
                AegisError::Vault("Wrong encryption key or unreadable encrypted database.".into())
            })?;
        Ok(conn)
    }

    /// Opens or creates an encrypted on-disk SQLCipher database with `key`, verifying decryption
    /// immediately before applying PRAGMAs and migrations.
    pub fn open_encrypted(path: &Path, key: &crypto::DbKey) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Self::open_encrypted_raw(path, key)?;
        Self::init_connection(conn, false, true)
    }

    /// Opens an isolated in-memory SQLite database for tests.
    pub fn open_in_memory() -> Result<Self> {
        register_extensions();
        let conn = Connection::open_in_memory()?;
        Self::init_connection(conn, true, false)
    }

    /// True when opened with [`SqliteStore::open_in_memory`] (test isolation checks).
    pub fn is_in_memory(&self) -> bool {
        self.in_memory
    }

    /// True when opened with [`SqliteStore::open_encrypted`].
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// Returns `(cipher_version, cipher_provider, vec_version)`.
    pub fn cipher_info(&self) -> Result<(String, String, String)> {
        let vec_ver: String = self
            .conn
            .query_row("SELECT vec_version()", [], |r| r.get(0))
            .unwrap_or_default();
        if !self.encrypted {
            return Ok((String::new(), String::new(), vec_ver));
        }
        let ver: String = self
            .conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))
            .unwrap_or_default();
        let prov: String = self
            .conn
            .query_row("PRAGMA cipher_provider", [], |r| r.get(0))
            .unwrap_or_default();
        Ok((ver, prov, vec_ver))
    }

    fn init_connection(conn: Connection, in_memory: bool, encrypted: bool) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -16384;",
        )?;
        let mut store = Self {
            conn,
            in_memory,
            encrypted,
        };
        store.run_migrations()?;
        Ok(store)
    }

    fn run_migrations(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )?;

        for (version, name, sql) in MIGRATIONS {
            let applied: Option<i64> = self
                .conn
                .query_row(
                    "SELECT version FROM schema_migrations WHERE version = ?1",
                    params![version],
                    |r| r.get(0),
                )
                .optional()?;
            if applied.is_none() {
                let tx = self.conn.transaction()?;
                tx.execute_batch(sql)?;
                tx.execute(
                    "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                    params![version, name, now_ms()],
                )?;
                tx.commit()?;
            }
        }
        Ok(())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Releases freeable SQLite page-cache memory after large scans or batches (`K5`).
    pub fn shrink_memory(&self) {
        let _ = self.conn.execute_batch("PRAGMA shrink_memory;");
    }

    /// Executes `f` inside an ACID transaction with `PRAGMA synchronous = FULL` (`B1/B2` §6).
    pub fn with_full_sync_tx<T, F>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&Transaction<'_>) -> Result<T>,
    {
        self.conn.execute_batch("PRAGMA synchronous = FULL;")?;
        let res = (|| {
            let tx = self.conn.transaction()?;
            let out = f(&tx)?;
            tx.commit()?;
            Ok(out)
        })();
        let _ = self.conn.execute_batch("PRAGMA synchronous = NORMAL;");
        res
    }

    pub fn set_setting(&self, key: &str, scope: &str, value_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, scope, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key, scope) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![key, scope, value_json, now_ms()],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str, scope: &str) -> Result<Option<String>> {
        let val = self
            .conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1 AND scope = ?2",
                params![key, scope],
                |r| r.get(0),
            )
            .optional()?;
        Ok(val)
    }

    /// Registers or refreshes a volume row based on live OS metadata.
    pub fn ensure_volume_for_path(&self, path: &Path) -> Result<VolumeInfo> {
        // Drives managed by the registry (passport / fingerprint ids) are matched by their
        // registered mount point, so the OS volume serial never mis-attributes their files.
        if let Some((vid, _, _)) = files::resolve_online_volume(&self.conn, path)?
            && let Some(v) = self
                .list_volumes()?
                .into_iter()
                .find(|v| v.id == vid && (v.passport_id.is_some() || v.id.starts_with("fp-")))
        {
            self.conn.execute(
                "UPDATE volumes SET last_seen = ?2 WHERE id = ?1",
                params![v.id, now_ms()],
            )?;
            return Ok(v);
        }
        let meta = inspect_volume_for_path(path)?;
        let quarantine_dir = crate::quarantine::quarantine_dir_for(Path::new(&meta.mount_point)).to_string_lossy().to_string();
        let ts = now_ms();
        self.conn.execute(
            "INSERT INTO volumes (id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode, case_sensitive, quarantine_dir, status, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'online', ?9)
             ON CONFLICT(id) DO UPDATE SET
               mount_point = excluded.mount_point,
               fs_type = excluded.fs_type,
               is_removable = excluded.is_removable,
               is_readonly = excluded.is_readonly,
               has_stable_inode = excluded.has_stable_inode,
               case_sensitive = excluded.case_sensitive,
               quarantine_dir = COALESCE(NULLIF(volumes.quarantine_dir, ''), excluded.quarantine_dir),
               status = 'online',
               last_seen = excluded.last_seen",
            params![
                meta.id,
                meta.mount_point,
                meta.fs_type,
                i32::from(meta.is_removable),
                i32::from(meta.is_readonly),
                i32::from(meta.has_stable_inode),
                i32::from(meta.case_sensitive),
                quarantine_dir,
                ts
            ],
        )?;

        Ok(VolumeInfo {
            id: meta.id,
            mount_point: meta.mount_point,
            fs_type: meta.fs_type,
            is_removable: meta.is_removable,
            is_readonly: meta.is_readonly,
            has_stable_inode: meta.has_stable_inode,
            case_sensitive: meta.case_sensitive,
            quarantine_dir: Some(quarantine_dir),
            status: "online".to_string(),
            last_seen: ts,
            total_bytes: meta.total_bytes,
            free_bytes: meta.free_bytes,
            label: None,
            role: "unassigned".to_string(),
            passport_id: None,
            last_catalog_complete_at: None,
        })
    }

    pub fn list_volumes(&self) -> Result<Vec<VolumeInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode, case_sensitive, quarantine_dir, status, last_seen,
                    label, role, passport_id, last_catalog_complete_at
             FROM volumes ORDER BY mount_point",
        )?;
        let rows = stmt.query_map([], |r| {
            let mount_point: String = r.get(1)?;
            let (total_bytes, free_bytes) = disk_space_for_mount(&mount_point);
            Ok(VolumeInfo {
                id: r.get(0)?,
                mount_point,
                fs_type: r.get(2)?,
                is_removable: r.get::<_, i32>(3)? != 0,
                is_readonly: r.get::<_, i32>(4)? != 0,
                has_stable_inode: r.get::<_, i32>(5)? != 0,
                case_sensitive: r.get::<_, i32>(6)? != 0,
                quarantine_dir: r.get(7)?,
                status: r.get(8)?,
                last_seen: r.get(9)?,
                total_bytes,
                free_bytes,
                label: r.get(10)?,
                role: r.get(11)?,
                passport_id: r.get(12)?,
                last_catalog_complete_at: r.get(13)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_roots(&self) -> Result<Vec<RootConfig>> {
        let raw = self.get_setting("roots", "global")?;
        match raw {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    pub fn save_roots(&self, roots: &[RootConfig]) -> Result<()> {
        let json = serde_json::to_string(roots)?;
        self.set_setting("roots", "global", &json)
    }

    pub fn add_root(&self, path: &Path) -> Result<RootConfig> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let display_path = canonical
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .unwrap_or(&canonical.to_string_lossy())
            .to_string();
        let vol = self.ensure_volume_for_path(&canonical)?;
        let mut roots = self.list_roots()?;
        if let Some(existing) = roots.iter().find(|r| r.path.eq_ignore_ascii_case(&display_path)) {
            return Ok(existing.clone());
        }
        let cfg = RootConfig {
            id: crate::new_id(),
            path: display_path,
            enabled: true,
            volume_id: vol.id,
        };
        roots.push(cfg.clone());
        self.save_roots(&roots)?;
        Ok(cfg)
    }

    pub fn remove_root(&self, path: &str) -> Result<()> {
        let mut roots = self.list_roots()?;
        roots.retain(|r| !r.path.eq_ignore_ascii_case(path));
        self.save_roots(&roots)
    }

    pub fn toggle_root(&self, path: &str, enabled: bool) -> Result<()> {
        let mut roots = self.list_roots()?;
        for root in &mut roots {
            if root.path.eq_ignore_ascii_case(path) {
                root.enabled = enabled;
            }
        }
        self.save_roots(&roots)
    }

    pub fn index_stats(&self) -> Result<IndexStats> {
        let total_files: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        let active_files: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND state IN ('active','warm','cold','archive')",
            [],
            |r| r.get(0),
        )?;
        let quarantined_files: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND state = 'quarantine'",
            [],
            |r| r.get(0),
        )?;
        let placeholder_files: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE deleted_at IS NULL AND cloud_state = 'placeholder_remote'",
            [],
            |r| r.get(0),
        )?;
        let total_chunks: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let fts_chunks: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM chunks_fts_docsize", [], |r| r.get(0))?;
        let atomic_units: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM atomic_units", [], |r| r.get(0))?;
        let cache_dirs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cache_dirs WHERE state = 'active'",
            [],
            |r| r.get(0),
        )?;
        let pending_outbox: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM outbox_events WHERE processed_at IS NULL",
            [],
            |r| r.get(0),
        )?;

        Ok(IndexStats {
            total_files,
            active_files,
            quarantined_files,
            placeholder_files,
            total_chunks,
            fts_chunks,
            atomic_units,
            cache_dirs,
            pending_outbox,
        })
    }
}
