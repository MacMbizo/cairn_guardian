//! Drives & Duplicates: drive identity, passport, lifecycle and catalog tests.

mod common;

use aegis_core::db::SqliteStore;
use rusqlite::{Connection, params};

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|c| c.unwrap())
        .collect()
}

fn versions(conn: &Connection) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(|c| c.unwrap())
        .collect()
}

#[test]
fn migration_0002_applies_on_fresh_and_v1_databases() {
    // Fresh database.
    let store = SqliteStore::open_in_memory().unwrap();
    assert_eq!(versions(store.conn()), vec![1, 2]);
    let vol_cols = columns(store.conn(), "volumes");
    for c in ["passport_id", "role", "guid_path", "label", "usn_journal_id", "last_catalog_complete_at"] {
        assert!(vol_cols.contains(&c.to_string()), "volumes.{c} missing");
    }
    assert!(columns(store.conn(), "files").contains(&"quick_hash".to_string()));
    assert!(columns(store.conn(), "actions").contains(&"survivor_file_id".to_string()));
    for t in [
        "dupe_decisions",
        "backup_sources",
        "backup_copies",
        "backup_versions",
        "standing_approvals",
        "backup_jobs",
    ] {
        assert!(!columns(store.conn(), t).is_empty(), "table {t} missing");
    }

    // Database created by v1 only, with an existing volume row.
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("v1.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(include_str!("../migrations/0001_initial_b4_v2_3_1.sql"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL);
             INSERT INTO schema_migrations VALUES (1, '0001_initial_b4_v2_3_1', 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO volumes (id, mount_point, fs_type, last_seen) VALUES ('win-1', 'E:\\', 'ntfs', ?1)",
            params![1i64],
        )
        .unwrap();
    }
    let store = SqliteStore::open(&db).unwrap();
    assert_eq!(versions(store.conn()), vec![1, 2]);
    let role: String = store
        .conn()
        .query_row("SELECT role FROM volumes WHERE id = 'win-1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(role, "unassigned");
    let vols = store.list_volumes().unwrap();
    assert_eq!(vols[0].role, "unassigned");
    assert_eq!(vols[0].passport_id, None);
}
