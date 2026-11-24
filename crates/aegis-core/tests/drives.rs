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
    assert_eq!(versions(store.conn()), vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
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
    assert_eq!(versions(store.conn()), vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
    let role: String = store
        .conn()
        .query_row("SELECT role FROM volumes WHERE id = 'win-1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(role, "unassigned");
    let vols = store.list_volumes().unwrap();
    assert_eq!(vols[0].role, "unassigned");
    assert_eq!(vols[0].passport_id, None);
}

use aegis_core::drives::{
    FakeVolumeProvider, VolumeProvider, read_passport, regenerate_passport, write_passport,
};

#[test]
fn passport_roundtrip_and_regenerate() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    assert!(read_passport(root).unwrap().is_none());

    let p = write_passport(root).unwrap();
    assert_eq!(p.schema, 1);
    assert!(p.created_by.starts_with("aegis "));
    assert!(root.join(".aegis").join("volume.json").is_file());
    assert_eq!(read_passport(root).unwrap(), Some(p.clone()));

    let p2 = regenerate_passport(root).unwrap();
    assert_ne!(p2.passport_id, p.passport_id);
    assert_eq!(read_passport(root).unwrap(), Some(p2));

    std::fs::write(root.join(".aegis").join("volume.json"), "{not json").unwrap();
    assert!(read_passport(root).unwrap().is_none());
}

#[test]
fn fake_provider_plug_unplug() {
    let tmp = tempfile::TempDir::new().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    provider.plug(b.clone());
    assert_eq!(provider.mounted().unwrap().len(), 2);
    provider.unplug(&a.mount_point);
    let now = provider.mounted().unwrap();
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].mount_point, b.mount_point);
}

#[cfg(windows)]
#[test]
fn windows_provider_lists_system_drive() {
    let vols = aegis_core::drives::WindowsProvider.mounted().unwrap();
    let sys = vols.iter().find(|v| v.is_system).expect("system drive listed");
    assert!(!sys.fs_type.is_empty());
    assert!(sys.total_bytes > 0);
}

use aegis_core::drives::sync_mounted;
use aegis_core::scanner::scan_root;

#[test]
fn new_drive_gets_passport_and_pp_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    let report = sync_mounted(&mut store, &provider).unwrap();
    assert_eq!(report.new_drives.len(), 1);
    let passport = read_passport(&a.mount_point).unwrap().expect("passport written");
    let id = common::volume_id_at(&store, &a.mount_point);
    assert_eq!(id, format!("pp-{}", passport.passport_id));
    let v = store.list_volumes().unwrap().into_iter().find(|v| v.id == id).unwrap();
    assert_eq!(v.label.as_deref(), Some("A"));
    assert_eq!(v.passport_id.as_deref(), Some(passport.passport_id.as_str()));

    // A second sync recognises the same drive instead of creating another.
    let again = sync_mounted(&mut store, &provider).unwrap();
    assert!(again.new_drives.is_empty());
    assert!(again.arrived.is_empty());
}

#[test]
fn unplug_marks_offline_without_touching_file_rows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "Photos/x.jpg", b"x");
    common::put(&a.mount_point, "doc.txt", b"doc");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    scan_root(&mut store, &a.mount_point, &[1]).unwrap();
    let id = common::volume_id_at(&store, &a.mount_point);
    assert_eq!(common::live_file_paths(&store, &id).len(), 2);

    provider.unplug(&a.mount_point);
    let report = sync_mounted(&mut store, &provider).unwrap();
    assert_eq!(report.departed, vec![id.clone()]);
    assert_eq!(common::status_of(&store, &id), "offline");
    assert_eq!(common::live_file_paths(&store, &id).len(), 2);
    let missing: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM files WHERE volume_id = ?1 AND (state = 'missing' OR deleted_at IS NOT NULL)",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 0);
}

#[test]
fn replug_under_new_letter_rebases_paths() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "Photos/x.jpg", b"x");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    scan_root(&mut store, &a.mount_point, &[1]).unwrap();
    let id = common::volume_id_at(&store, &a.mount_point);

    provider.unplug(&a.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let new_mount = tmp.path().join("F");
    std::fs::rename(&a.mount_point, &new_mount).unwrap();
    let mut moved = a.clone();
    moved.mount_point = new_mount.clone();
    provider.plug(moved);
    let report = sync_mounted(&mut store, &provider).unwrap();
    assert_eq!(report.relettered.len(), 1);
    assert_eq!(common::volume_id_at(&store, &new_mount), id);

    let new_prefix = common::canon(&new_mount);
    let mut stmt = store
        .conn()
        .prepare("SELECT path, path_hash FROM files WHERE volume_id = ?1")
        .unwrap();
    let rows: Vec<(String, Vec<u8>)> = stmt
        .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    for (path, hash) in rows {
        assert!(path.starts_with(&new_prefix), "{path} not under {new_prefix}");
        let (_, h) = aegis_core::db::files::normalize_path(std::path::Path::new(&path), false);
        assert_eq!(hash, h.to_vec());
    }
}

#[test]
fn letter_reused_by_other_drive_marks_old_offline() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "M", "0001");
    common::put(&a.mount_point, "a-only.txt", b"a");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    scan_root(&mut store, &a.mount_point, &[1]).unwrap();
    let a_id = common::volume_id_at(&store, &a.mount_point);

    // Drive A leaves; a different drive B appears at the same mount point.
    std::fs::rename(&a.mount_point, tmp.path().join("A-elsewhere")).unwrap();
    std::fs::create_dir_all(&a.mount_point).unwrap();
    common::put(&a.mount_point, "b-only.txt", b"b");
    let mut b = a.clone();
    b.serial = Some("0002".into());
    b.guid_path = Some(r"\\?\Volume{0002-guid}\".into());
    b.label = Some("B".into());
    provider.set(vec![b.clone()]);
    let report = sync_mounted(&mut store, &provider).unwrap();
    assert_eq!(report.new_drives.len(), 1);
    assert_eq!(common::status_of(&store, &a_id), "offline");
    let b_id = common::volume_id_at(&store, &b.mount_point);
    assert_ne!(a_id, b_id);

    scan_root(&mut store, &b.mount_point, &[1]).unwrap();
    let b_files = common::live_file_paths(&store, &b_id);
    assert_eq!(b_files.len(), 1);
    assert!(b_files[0].ends_with("b-only.txt"));
    let a_files = common::live_file_paths(&store, &a_id);
    assert_eq!(a_files.len(), 1);
    assert!(a_files[0].ends_with("a-only.txt"));
}

#[test]
fn cloned_drive_gets_new_passport() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    let a_id = common::volume_id_at(&store, &a.mount_point);
    let a_passport = read_passport(&a.mount_point).unwrap().unwrap();

    let c = common::fake_drive(&tmp, "C", "0001"); // clones share serials too
    std::fs::create_dir_all(c.mount_point.join(".aegis")).unwrap();
    std::fs::copy(
        a.mount_point.join(".aegis").join("volume.json"),
        c.mount_point.join(".aegis").join("volume.json"),
    )
    .unwrap();
    provider.plug(c.clone());
    let report = sync_mounted(&mut store, &provider).unwrap();
    assert_eq!(report.clones.len(), 1);
    let c_passport = read_passport(&c.mount_point).unwrap().unwrap();
    assert_ne!(c_passport.passport_id, a_passport.passport_id);
    assert_eq!(read_passport(&a.mount_point).unwrap().unwrap(), a_passport);
    assert_eq!(common::volume_id_at(&store, &a.mount_point), a_id);
    assert_ne!(common::volume_id_at(&store, &c.mount_point), a_id);
}

#[test]
fn readonly_drive_uses_fingerprint_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut r = common::fake_drive(&tmp, "R", "00ab");
    r.is_readonly = true;
    let provider = FakeVolumeProvider::new();
    provider.plug(r.clone());
    sync_mounted(&mut store, &provider).unwrap();
    assert!(!r.mount_point.join(".aegis").exists());
    assert!(common::volume_id_at(&store, &r.mount_point).starts_with("fp-"));
    // Recognised again on the next sync.
    assert!(sync_mounted(&mut store, &provider).unwrap().new_drives.is_empty());
}

use aegis_core::scanner::{catalog_volume, seed_default_roots};

#[test]
fn catalog_reads_no_payload_and_skips_system_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "a.txt", b"hello");
    common::put(&a.mount_point, "System Volume Information/x", b"x");
    common::put(&a.mount_point, "$RECYCLE.BIN/y", b"y");
    common::put(&a.mount_point, "Aegis Backup/_Previous versions/2026-01-01/z", b"z");
    common::put(&a.mount_point, "Aegis Backup/Seagate A/kept.txt", b"k");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    let id = common::volume_id_at(&store, &a.mount_point);

    catalog_volume(&mut store, &id, &mut |_| {}).unwrap();

    let names: Vec<String> = common::live_file_paths(&store, &id)
        .into_iter()
        .map(|p| p.rsplit(['\\', '/']).next().unwrap().to_string())
        .collect();
    let mut names_sorted = names.clone();
    names_sorted.sort();
    assert_eq!(names_sorted, vec!["a.txt".to_string(), "kept.txt".to_string()]);
    let hashed: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM files WHERE volume_id = ?1 AND blake3 IS NOT NULL",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hashed, 0, "a catalog must not read file contents");
    let v = store.list_volumes().unwrap().into_iter().find(|v| v.id == id).unwrap();
    assert!(v.last_catalog_complete_at.is_some());
}

#[test]
fn catalog_offline_volume_errors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    let id = common::volume_id_at(&store, &a.mount_point);
    provider.unplug(&a.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let err = catalog_volume(&mut store, &id, &mut |_| {}).unwrap_err();
    assert!(matches!(err, aegis_core::error::AegisError::VolumeOffline(_)), "{err:?}");
}

#[test]
fn seed_default_roots_only_when_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    store.add_root(tmp.path()).unwrap();
    assert!(seed_default_roots(&store).unwrap().is_empty());
    assert_eq!(store.list_roots().unwrap().len(), 1);

    let fresh = SqliteStore::open_in_memory().unwrap();
    let seeded = seed_default_roots(&fresh).unwrap();
    assert_eq!(fresh.list_roots().unwrap().len(), seeded.len());
    for r in &seeded {
        assert!(std::path::Path::new(&r.path).is_dir(), "{} should exist", r.path);
    }
    // Idempotent: a second call adds nothing.
    assert!(seed_default_roots(&fresh).unwrap().is_empty());
}

use aegis_core::drives::{RefreshMode, refresh_volume};
use std::sync::atomic::AtomicBool;

fn file_id_at(store: &SqliteStore, path: &std::path::Path) -> Option<String> {
    use rusqlite::OptionalExtension;
    store
        .conn()
        .query_row(
            "SELECT id FROM files WHERE path = ?1 AND deleted_at IS NULL",
            [common::canon(path)],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}

#[test]
fn walk_refresh_detects_create_modify_delete_rename() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let provider = FakeVolumeProvider::new();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "keep.txt", b"keep");
    let m = common::put(&a.mount_point, "docs/mod.txt", b"old");
    let d = common::put(&a.mount_point, "del.txt", b"gone soon");
    let r = common::put(&a.mount_point, "ren.txt", b"rename me");
    let id = common::plug_and_catalog(&mut store, &provider, &a);
    let ren_id = file_id_at(&store, &r).unwrap();

    common::put(&a.mount_point, "new.txt", b"brand new");
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&m, b"new and longer").unwrap();
    std::fs::remove_file(&d).unwrap();
    let r2 = a.mount_point.join("renamed.txt");
    std::fs::rename(&r, &r2).unwrap();

    let report = refresh_volume(&mut store, &id, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.mode, RefreshMode::Walk);
    assert_eq!(report.created, 1, "{report:?}");
    assert_eq!(report.modified, 1, "{report:?}");
    assert_eq!(report.deleted, 1, "{report:?}");
    assert_eq!(report.renamed, 1, "{report:?}");
    assert!(report.folders_changed >= 2);
    assert_eq!(file_id_at(&store, &r2), Some(ren_id), "rename keeps the file's identity");
    assert_eq!(file_id_at(&store, &d), None);
}

#[test]
fn modified_file_loses_stale_hashes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let provider = FakeVolumeProvider::new();
    let a = common::fake_drive(&tmp, "A", "0001");
    let p1 = common::put(&a.mount_point, "a.txt", b"same bytes");
    common::put(&a.mount_point, "b.txt", b"same bytes");
    let id = common::plug_and_catalog(&mut store, &provider, &a);
    aegis_core::fingerprint::run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert!(common::hashes_at(&store, &p1).1.is_some());

    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&p1, b"diff bytes").unwrap(); // same size, new content
    refresh_volume(&mut store, &id, &AtomicBool::new(false)).unwrap();
    let (_, b) = common::hashes_at(&store, &p1);
    assert_ne!(b, Some(blake3::hash(b"same bytes").as_bytes().to_vec()), "stale hash kept");
}

#[test]
fn refresh_reuses_hashes_for_unchanged_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let provider = FakeVolumeProvider::new();
    let a = common::fake_drive(&tmp, "A", "0001");
    let p1 = common::put(&a.mount_point, "a.bin", &common::noise(4096, 7));
    common::put(&a.mount_point, "b.bin", &common::noise(4096, 7));
    let id = common::plug_and_catalog(&mut store, &provider, &a);
    aegis_core::fingerprint::run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    let before = common::hashes_at(&store, &p1);

    let report = refresh_volume(&mut store, &id, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.fingerprint.quick_hashed, 0);
    assert_eq!(report.created + report.modified + report.deleted + report.renamed, 0);
    assert_eq!(common::hashes_at(&store, &p1), before);
}

#[test]
fn usn_reset_falls_back_to_walk() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let provider = FakeVolumeProvider::new();
    let a = common::fake_drive(&tmp, "A", "0001");
    let id = common::plug_and_catalog(&mut store, &provider, &a);
    store
        .conn()
        .execute(
            "UPDATE volumes SET usn_journal_id = 'bogus', usn_next = 1 WHERE id = ?1",
            [&id],
        )
        .unwrap();
    let report = refresh_volume(&mut store, &id, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.mode, RefreshMode::Walk);
}

#[cfg(windows)]
#[test]
#[ignore = "needs an NTFS volume with a change journal; run manually"]
fn usn_refresh_on_system_temp() {
    use aegis_core::drives::usn::{UsnRead, query_journal, read_changes};
    let tmp = tempfile::TempDir::new().unwrap();
    let mount = std::path::PathBuf::from(&common::canon(tmp.path())[..3]); // e.g. C:\
    let (journal, next) = query_journal(&mount).unwrap().expect("journal active");
    let f = common::put(tmp.path(), "usn-probe.txt", b"probe");
    let renamed = tmp.path().join("usn-probe-renamed.txt");
    std::fs::rename(&f, &renamed).unwrap();
    let gone = common::put(tmp.path(), "usn-gone.txt", b"bye");
    std::fs::remove_file(&gone).unwrap();
    match read_changes(&mount, &journal, next).unwrap() {
        UsnRead::Changes(changes) => {
            let want = common::canon(&renamed).to_lowercase();
            let resolved: Vec<String> = changes
                .iter()
                .filter_map(|c| c.path.as_ref())
                .map(|p| p.to_string_lossy().to_lowercase())
                .collect();
            assert!(resolved.contains(&want), "renamed file resolved by reference: {want}");
            assert!(changes.iter().any(|c| c.path.is_none()), "deleted file has no path");
        }
        UsnRead::Reset => panic!("journal reported reset"),
    }
}

/// Review I1: an old, unmanaged volume row at the same mount point goes offline when a
/// registered drive claims that mount, and a catalog files everything under the claimed drive.
#[test]
fn claimed_mount_retires_legacy_rows_and_catalog_uses_the_drive_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "f.txt", b"file");
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, fs_type, status, last_seen) VALUES ('win-legacy', ?1, 'ntfs', 'online', 0)",
            [common::canon(&a.mount_point)],
        )
        .unwrap();
    let provider = FakeVolumeProvider::new();
    let id = common::plug_and_catalog(&mut store, &provider, &a);
    assert_eq!(common::status_of(&store, "win-legacy"), "offline");
    assert_eq!(common::live_file_paths(&store, &id).len(), 1);
    assert_eq!(common::live_file_paths(&store, "win-legacy").len(), 0);
}

#[test]
fn relettered_drive_rebases_context_history_jobs_samples_roots_and_actions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "Work/doc.txt", b"hello");
    let provider = FakeVolumeProvider::new();
    provider.plug(a.clone());
    sync_mounted(&mut store, &provider).unwrap();
    scan_root(&mut store, &a.mount_point, &[1]).unwrap();
    let id = common::volume_id_at(&store, &a.mount_point);
    store.add_root(&a.mount_point).unwrap();

    let old_mount = common::canon(&a.mount_point);
    let file_id: String = store
        .conn()
        .query_row("SELECT id FROM files WHERE volume_id = ?1 LIMIT 1", [&id], |r| r.get(0))
        .unwrap();
    let old_file = format!("{old_mount}\\Work\\doc.txt");
    let new_file = format!("{old_mount}\\Work\\doc2.txt");

    // Seed all additional path-bearing tables on drive `a`.
    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, confidence, state, computed_at) VALUES ('col1', 'project', 'Proj', 0.9, 'confirmed', 1)",
            [],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
             VALUES ('loc1', 'col1', ?1, ?2, 'home', 1, 5, 1, 0, 'content', 0, 'fp1')",
            rusqlite::params![id, format!("{old_mount}\\Work")],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO action_batches (id, origin, status, total_actions, created_at) VALUES ('b1', 'filing:pc', 'draft', 1, 1)",
            [],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO actions (id, batch_id, action_type, target_kind, file_id, from_path, to_path, reason, confidence, evidence_json, status, created_at)
             VALUES ('act1', 'b1', 'move', 'file', ?1, ?2, ?3, 'test', 0.9, '{}', 'proposed', 1)",
            rusqlite::params![file_id, old_file, new_file],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO file_history (id, file_id, old_path, new_path, batch_id, changed_at) VALUES ('fh1', ?1, ?2, ?3, 'b1', 1)",
            rusqlite::params![file_id, old_file, new_file],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO root_samples (root, day, bytes) VALUES (?1, '2026-11-23', 100)",
            [&old_mount],
        )
        .unwrap();

    // Unplug `A` and replug under `G`.
    provider.unplug(&a.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let g_mount = tmp.path().join("G");
    std::fs::rename(&a.mount_point, &g_mount).unwrap();
    let mut moved = a.clone();
    moved.mount_point = g_mount.clone();
    provider.plug(moved);
    sync_mounted(&mut store, &provider).unwrap();

    let new_mount = common::canon(&g_mount);
    let loc_path: String = store.conn().query_row("SELECT root_path FROM collection_locations WHERE id = 'loc1'", [], |r| r.get(0)).unwrap();
    assert!(loc_path.starts_with(&new_mount), "collection_locations: {loc_path}");
    let (h_old, h_new): (String, String) = store.conn().query_row("SELECT old_path, new_path FROM file_history WHERE id = 'fh1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(h_old.starts_with(&new_mount) && h_new.starts_with(&new_mount), "{h_old} -> {h_new}");
    let (a_from, a_to): (String, Option<String>) = store.conn().query_row("SELECT from_path, to_path FROM actions WHERE id = 'act1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(a_from.starts_with(&new_mount) && a_to.unwrap().starts_with(&new_mount), "{a_from}");
    let sample_root: String = store.conn().query_row("SELECT root FROM root_samples WHERE day = '2026-11-23'", [], |r| r.get(0)).unwrap();
    assert_eq!(sample_root, new_mount);
    let jobs: Vec<String> = {
        let mut st = store.conn().prepare("SELECT root_path FROM scan_jobs").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert!(jobs.iter().all(|p| p.starts_with(&new_mount)), "{jobs:?}");
    let roots = store.list_roots().unwrap();
    assert!(roots.iter().any(|r| r.volume_id == id && r.path == new_mount), "{roots:?}");
}
