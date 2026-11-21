//! Drives & Duplicates: fingerprinting, grouped duplicates, safety rules and decisions.

mod common;

use aegis_core::db::SqliteStore;
use aegis_core::drives::{FakeVolumeProvider, sync_mounted};
use aegis_core::fingerprint::run_fingerprint;
use std::sync::atomic::AtomicBool;

const KIB: usize = 1024;

fn setup() -> (tempfile::TempDir, SqliteStore, FakeVolumeProvider) {
    (
        tempfile::TempDir::new().unwrap(),
        SqliteStore::open_in_memory().unwrap(),
        FakeVolumeProvider::new(),
    )
}

#[test]
fn unique_sizes_are_never_read() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "a", &[1u8; 10]);
    common::put(&a.mount_point, "b", &[1u8; 11]);
    common::put(&a.mount_point, "c", &[1u8; 12]);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.size_candidates, 0);
    assert_eq!(stats.quick_hashed, 0);
    assert_eq!(stats.full_hashed, 0);
}

#[test]
fn same_size_different_content_stops_at_quick_hash() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(300 * KIB, 1);
    let mut other = body.clone();
    other[0] ^= 0xff;
    common::put(&a.mount_point, "one.bin", &body);
    common::put(&a.mount_point, "two.bin", &other);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.size_candidates, 2);
    assert_eq!(stats.quick_hashed, 2);
    assert_eq!(stats.full_hashed, 0);
}

#[test]
fn same_size_same_edges_different_middle_gets_full_hash() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(300 * KIB, 2);
    let mut other = body.clone();
    other[150 * KIB] ^= 0xff; // outside the first and last 64 KiB
    let p1 = common::put(&a.mount_point, "one.bin", &body);
    let p2 = common::put(&a.mount_point, "two.bin", &other);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.full_hashed, 2);
    let (q1, b1) = common::hashes_at(&store, &p1);
    let (q2, b2) = common::hashes_at(&store, &p2);
    assert_eq!(q1, q2, "edges are identical");
    assert!(b1.is_some() && b2.is_some());
    assert_ne!(b1, b2);
    assert_eq!(b1.unwrap(), blake3::hash(&body).as_bytes().to_vec());
}

#[test]
fn small_files_get_full_hash_from_quick_pass() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(KIB, 3);
    let p1 = common::put(&a.mount_point, "x.txt", &body);
    let p2 = common::put(&a.mount_point, "sub/x.txt", &body);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.full_hashed, 0);
    let expected = blake3::hash(&body).as_bytes().to_vec();
    assert_eq!(common::hashes_at(&store, &p1).1, Some(expected.clone()));
    assert_eq!(common::hashes_at(&store, &p2).1, Some(expected));
}

#[test]
fn offline_partner_triggers_full_hash_of_online_file() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(300 * KIB, 4);
    common::put(&a.mount_point, "x.bin", &body);
    common::put(&a.mount_point, "copy/x.bin", &body);
    common::plug_and_catalog(&mut store, &provider, &a);
    run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();

    provider.unplug(&a.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let b = common::fake_drive(&tmp, "B", "0002");
    let pb = common::put(&b.mount_point, "x.bin", &body);
    common::plug_and_catalog(&mut store, &provider, &b);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.quick_hashed, 1, "only B's new file is read");
    assert_eq!(stats.full_hashed, 1);
    assert_eq!(stats.skipped_offline, 0, "A's rows were already fingerprinted");
    assert_eq!(
        common::hashes_at(&store, &pb).1,
        Some(blake3::hash(&body).as_bytes().to_vec())
    );
}

#[test]
fn placeholders_are_never_opened() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(2 * KIB, 5);
    let p1 = common::put(&a.mount_point, "cloud.bin", &body);
    common::put(&a.mount_point, "local.bin", &body);
    common::put(&a.mount_point, "local2.bin", &body);
    common::plug_and_catalog(&mut store, &provider, &a);
    store
        .conn()
        .execute(
            "UPDATE files SET cloud_state = 'placeholder_remote' WHERE path = ?1",
            [common::canon(&p1)],
        )
        .unwrap();
    std::fs::remove_file(&p1).unwrap(); // reading it would fail loudly
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.skipped_placeholder, 1);
    let (q, _) = store
        .conn()
        .query_row(
            "SELECT quick_hash, blake3 FROM files WHERE cloud_state = 'placeholder_remote'",
            [],
            |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .unwrap();
    assert!(q.is_none());
}

#[test]
fn cancel_returns_partial_stats() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    for i in 0..5 {
        common::put(&a.mount_point, &format!("f{i}.bin"), &common::noise(KIB, 10 + i));
    }
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(true)).unwrap();
    assert_eq!(stats.size_candidates, 5);
    assert_eq!(stats.quick_hashed, 0);
}

// ---------------------------------------------------------------- Task 7: grouping + safety

use aegis_core::db::actions::{approve_batch, execute_batch, propose_dupe_quarantine};
use aegis_core::dupes::{DupeGroupKind, copy_count, grouped_report};
use std::path::Path;

/// Catalogs and fingerprints every given drive.
fn catalog_all(
    store: &mut SqliteStore,
    provider: &FakeVolumeProvider,
    drives: &[&aegis_core::drives::MountedVolume],
) -> Vec<String> {
    let ids = drives
        .iter()
        .map(|d| common::plug_and_catalog(store, provider, d))
        .collect();
    run_fingerprint(store, &AtomicBool::new(false)).unwrap();
    ids
}

fn file_id(store: &SqliteStore, path: &Path) -> String {
    store
        .conn()
        .query_row(
            "SELECT id FROM files WHERE path = ?1 AND deleted_at IS NULL",
            [common::canon(path)],
            |r| r.get(0),
        )
        .unwrap()
}

fn action_row(store: &SqliteStore, batch: &str) -> (String, Option<String>) {
    store
        .conn()
        .query_row(
            "SELECT status, error_message FROM actions WHERE batch_id = ?1",
            [batch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

fn set_mtime(path: &Path, secs_ago: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago))
        .unwrap();
}

#[test]
fn identical_folders_on_two_drives_form_one_folder_group() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let c = common::fake_drive(&tmp, "C", "0003");
    for (i, n) in ["a.jpg", "b.jpg", "c.jpg"].iter().enumerate() {
        let body = common::noise(2 * KIB + i, 20 + i as u64);
        common::put(&a.mount_point, &format!("Photos/2019/{n}"), &body);
        common::put(&c.mount_point, &format!("Old/Photos/2019/{n}"), &body);
    }
    common::put(&a.mount_point, "unrelated-a.txt", b"only on a");
    common::put(&c.mount_point, "unrelated-c.txt", b"only on c, longer");
    catalog_all(&mut store, &provider, &[&a, &c]);

    let report = grouped_report(store.conn()).unwrap();
    assert_eq!(report.groups.len(), 1, "{:#?}", report.groups);
    let g = &report.groups[0];
    assert_eq!(g.kind, DupeGroupKind::Folder);
    assert_eq!(g.file_count, 3);
    assert_eq!(g.locations.len(), 2);
    let paths: Vec<&str> = g.locations.iter().map(|l| l.path.as_str()).collect();
    assert!(paths.iter().any(|p| p.ends_with("A\\Photos") || p.ends_with("A/Photos")), "{paths:?}");
    assert!(g.needs_question);
}

#[test]
fn ninety_percent_overlap_is_partial_with_differences() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let c = common::fake_drive(&tmp, "C", "0003");
    for i in 0..10u64 {
        let body = common::noise(3 * KIB + i as usize, 40 + i);
        common::put(&a.mount_point, &format!("Set/f{i}.bin"), &body);
        if i < 9 {
            common::put(&c.mount_point, &format!("Copy/Set/f{i}.bin"), &body);
        }
    }
    common::put(&c.mount_point, "Copy/Set/extra.bin", b"extra file");
    common::put(&a.mount_point, "root-a.txt", b"a root");
    common::put(&c.mount_point, "root-c.txt", b"c root file");
    catalog_all(&mut store, &provider, &[&a, &c]);

    let report = grouped_report(store.conn()).unwrap();
    assert_eq!(report.groups.len(), 1, "{:#?}", report.groups);
    let g = &report.groups[0];
    assert_eq!(g.kind, DupeGroupKind::PartialFolder);
    let mut diffs = g.differences.clone();
    diffs.sort();
    assert_eq!(diffs, vec!["extra.bin".to_string(), "f9.bin".to_string()]);
}

#[test]
fn keeper_prefers_non_backup_then_oldest() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let c = common::fake_drive(&tmp, "C", "0003");
    let body = common::noise(5 * KIB, 60);
    let pa = common::put(&a.mount_point, "x.bin", &body);
    let pb = common::put(&b.mount_point, "x.bin", &body);
    let pc = common::put(&c.mount_point, "x.bin", &body);
    set_mtime(&pb, 3000); // oldest, but on the Backup drive
    set_mtime(&pa, 2000);
    set_mtime(&pc, 1000);
    let ids = catalog_all(&mut store, &provider, &[&a, &b, &c]);
    store
        .conn()
        .execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&ids[1]])
        .unwrap();

    let report = grouped_report(store.conn()).unwrap();
    assert_eq!(report.groups.len(), 1);
    let g = &report.groups[0];
    let keeper = &g.locations[g.keeper_index];
    assert_eq!(keeper.volume_id, ids[0], "A: not backup, and older than C");
    assert!(g.locations.iter().any(|l| l.is_backup));
}

#[test]
fn refuses_delete_below_min_copies() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let c = common::fake_drive(&tmp, "C", "0003");
    let body = common::noise(5 * KIB, 61);
    let pa = common::put(&a.mount_point, "x.bin", &body);
    let pc = common::put(&c.mount_point, "x.bin", &body);
    catalog_all(&mut store, &provider, &[&a, &c]);
    let keep = file_id(&store, &pa);
    let drop = file_id(&store, &pc);
    let err = propose_dupe_quarantine(&mut store, &keep, &[drop]).unwrap_err();
    assert!(
        matches!(err, aegis_core::error::AegisError::PolicyBlocked { .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("Make this the backup copy"), "{err}");
}

#[test]
fn hardlinks_count_once() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let c = common::fake_drive(&tmp, "C", "0003");
    let body = common::noise(5 * KIB, 62);
    let pa = common::put(&a.mount_point, "x.bin", &body);
    std::fs::hard_link(&pa, a.mount_point.join("x-link.bin")).unwrap();
    common::put(&c.mount_point, "x.bin", &body);
    catalog_all(&mut store, &provider, &[&a, &c]);
    assert_eq!(copy_count(store.conn(), blake3::hash(&body).as_bytes()).unwrap(), 2);
}

fn three_copies(
    tmp: &tempfile::TempDir,
    store: &mut SqliteStore,
    provider: &FakeVolumeProvider,
) -> (std::path::PathBuf, std::path::PathBuf, aegis_core::drives::MountedVolume, String) {
    let a = common::fake_drive(tmp, "A", "0001");
    let b = common::fake_drive(tmp, "B", "0002");
    let c = common::fake_drive(tmp, "C", "0003");
    let body = common::noise(5 * KIB, 63);
    let pa = common::put(&a.mount_point, "x.bin", &body);
    common::put(&b.mount_point, "x.bin", &body);
    let pc = common::put(&c.mount_point, "x.bin", &body);
    catalog_all(store, provider, &[&a, &b, &c]);
    let keep = file_id(store, &pc);
    let drop = file_id(store, &pa);
    let batch = propose_dupe_quarantine(store, &keep, &[drop]).unwrap();
    approve_batch(store, &batch, None).unwrap();
    (pa, pc, c, batch)
}

#[test]
fn survivor_offline_fails_action_without_moving() {
    let (tmp, mut store, provider) = setup();
    let (pa, _pc, c, batch) = three_copies(&tmp, &mut store, &provider);
    provider.unplug(&c.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    execute_batch(&mut store, &batch).unwrap();
    let (status, msg) = action_row(&store, &batch);
    assert_eq!(status, "failed");
    let msg = msg.unwrap_or_default();
    assert!(msg.contains("Plug it in to confirm"), "{msg}");
    assert!(msg.contains("C"), "names the drive: {msg}");
    assert!(pa.exists(), "nothing moved");
}

#[test]
fn survivor_deleted_outside_aegis_fails() {
    let (tmp, mut store, provider) = setup();
    let (pa, pc, _c, batch) = three_copies(&tmp, &mut store, &provider);
    std::fs::remove_file(&pc).unwrap();
    execute_batch(&mut store, &batch).unwrap();
    let (status, _) = action_row(&store, &batch);
    assert_eq!(status, "failed");
    assert!(pa.exists(), "nothing moved");
}

#[test]
fn backup_copy_needs_no_question() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let body = common::noise(5 * KIB, 64);
    let pa = common::put(&a.mount_point, "x.bin", &body);
    common::put(&b.mount_point, "Aegis Backup/A/x.bin", &body);
    let ids = catalog_all(&mut store, &provider, &[&a, &b]);
    store
        .conn()
        .execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&ids[1]])
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO backup_copies (id, source_file_id, backup_volume_id, backup_rel_path, source_blake3, copied_at)
             VALUES ('bc1', ?1, ?2, ?3, ?4, 0)",
            rusqlite::params![
                file_id(&store, &pa),
                ids[1],
                Path::new("Aegis Backup").join("A").join("x.bin").to_string_lossy(),
                blake3::hash(&body).as_bytes().to_vec()
            ],
        )
        .unwrap();
    let report = grouped_report(store.conn()).unwrap();
    assert_eq!(report.groups.len(), 1);
    assert!(!report.groups[0].needs_question);
}

// ---------------------------------------------------------------- Task 8: decisions

use aegis_core::dupes::decisions::{DupeChoice, decide, list_decisions, undo_decision};

fn two_drive_copy(
    tmp: &tempfile::TempDir,
    store: &mut SqliteStore,
    provider: &FakeVolumeProvider,
) -> (aegis_core::drives::MountedVolume, aegis_core::drives::MountedVolume) {
    let a = common::fake_drive(tmp, "A", "0001");
    let c = common::fake_drive(tmp, "C", "0003");
    for i in 0..3u64 {
        let body = common::noise(2 * KIB + i as usize, 80 + i);
        common::put(&a.mount_point, &format!("Docs/f{i}.bin"), &body);
        common::put(&c.mount_point, &format!("Docs/f{i}.bin"), &body);
    }
    common::put(&a.mount_point, "a-only.txt", b"a");
    common::put(&c.mount_point, "c-only.txt", b"cc");
    catalog_all(store, provider, &[&a, &c]);
    (a, c)
}

#[test]
fn keep_both_suppresses_question_across_relettering() {
    let (tmp, mut store, provider) = setup();
    let (_a, c) = two_drive_copy(&tmp, &mut store, &provider);
    let g = grouped_report(store.conn()).unwrap().groups.remove(0);
    assert!(g.needs_question);
    let out = decide(&mut store, &g.group_id, DupeChoice::KeepBoth).unwrap();
    assert!(out.batch_id.is_none());
    assert!(!grouped_report(store.conn()).unwrap().groups[0].needs_question);

    // Drive C comes back under another letter: still answered.
    provider.unplug(&c.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let moved = tmp.path().join("C2");
    std::fs::rename(&c.mount_point, &moved).unwrap();
    let mut c2 = c.clone();
    c2.mount_point = moved;
    provider.plug(c2);
    sync_mounted(&mut store, &provider).unwrap();
    let after = grouped_report(store.conn()).unwrap();
    assert_eq!(after.groups.len(), 1);
    assert_eq!(after.groups[0].group_id, g.group_id, "group id survives a re-letter");
    assert!(!after.groups[0].needs_question);
    assert_eq!(list_decisions(store.conn()).unwrap().len(), 1);
}

#[test]
fn delete_this_copy_creates_quarantine_batch() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let c = common::fake_drive(&tmp, "C", "0003");
    let body = common::noise(5 * KIB, 90);
    for d in [&a, &b, &c] {
        common::put(&d.mount_point, "x.bin", &body);
    }
    catalog_all(&mut store, &provider, &[&a, &b, &c]);
    let g = grouped_report(store.conn()).unwrap().groups.remove(0);
    let victim = (0..g.locations.len()).find(|&i| i != g.keeper_index).unwrap();
    let out = decide(&mut store, &g.group_id, DupeChoice::DeleteThisCopy { location_index: victim }).unwrap();
    let batch = out.batch_id.expect("a quarantine batch");
    let (n, with_survivor): (i64, i64) = store
        .conn()
        .query_row(
            "SELECT COUNT(*), COUNT(survivor_file_id) FROM actions WHERE batch_id = ?1 AND action_type = 'quarantine'",
            [&batch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((n, with_survivor), (1, 1));
}

#[test]
fn delete_refused_suggests_backup_copy() {
    let (tmp, mut store, provider) = setup();
    two_drive_copy(&tmp, &mut store, &provider);
    let g = grouped_report(store.conn()).unwrap().groups.remove(0);
    let victim = 1 - g.keeper_index;
    let err = decide(&mut store, &g.group_id, DupeChoice::DeleteThisCopy { location_index: victim })
        .unwrap_err();
    assert!(err.to_string().contains("Make this the backup copy"), "{err}");
    assert!(list_decisions(store.conn()).unwrap().is_empty(), "a refused answer is not recorded");
}

#[test]
fn undo_decision_reasks() {
    let (tmp, mut store, provider) = setup();
    two_drive_copy(&tmp, &mut store, &provider);
    let g = grouped_report(store.conn()).unwrap().groups.remove(0);
    let out = decide(&mut store, &g.group_id, DupeChoice::KeepBoth).unwrap();
    undo_decision(&mut store, &out.decision_id).unwrap();
    assert!(grouped_report(store.conn()).unwrap().groups[0].needs_question);
    assert!(list_decisions(store.conn()).unwrap().is_empty());
}
