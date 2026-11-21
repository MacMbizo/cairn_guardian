//! Drives & Duplicates: drive roles, drive plan, backup mirror, guard and priority fill.

mod common;

use aegis_core::backup::plan::{apply_plan, recommend_plan};
use aegis_core::contracts::DriveRole;
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch};
use aegis_core::drives::{FakeVolumeProvider, MountedVolume, sync_mounted};
use aegis_core::fingerprint::run_fingerprint;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const KIB: usize = 1024;

fn setup() -> (tempfile::TempDir, SqliteStore, FakeVolumeProvider) {
    (
        tempfile::TempDir::new().unwrap(),
        SqliteStore::open_in_memory().unwrap(),
        FakeVolumeProvider::new(),
    )
}

fn catalog_all(store: &mut SqliteStore, provider: &FakeVolumeProvider, drives: &[&MountedVolume]) -> Vec<String> {
    let ids = drives
        .iter()
        .map(|d| common::plug_and_catalog(store, provider, d))
        .collect();
    run_fingerprint(store, &AtomicBool::new(false)).unwrap();
    ids
}

fn run_batch(store: &mut SqliteStore, batch: &str) {
    approve_batch(store, batch, None).unwrap();
    let res = execute_batch(store, batch).unwrap();
    let failed: Vec<(String, Option<String>)> = {
        let mut stmt = store
            .conn()
            .prepare("SELECT from_path, error_message FROM actions WHERE batch_id = ?1 AND status = 'failed'")
            .unwrap();
        stmt.query_map([batch], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(failed.is_empty(), "failed actions: {failed:?} ({res:?})");
}

fn count(store: &SqliteStore, sql: &str) -> i64 {
    store.conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn plan_not_ready_until_all_catalogued() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    common::plug_and_catalog(&mut store, &provider, &a);
    provider.plug(b.clone());
    sync_mounted(&mut store, &provider).unwrap();
    assert!(!recommend_plan(store.conn()).unwrap().ready);
    let b_id = common::volume_id_at(&store, &b.mount_point);
    aegis_core::scanner::catalog_volume(&mut store, &b_id, &mut |_| {}).unwrap();
    let plan = recommend_plan(store.conn()).unwrap();
    assert!(plan.ready);
    assert_eq!(plan.entries.len(), 2);
}

#[test]
fn recommends_drive_holding_most_backup_copies() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let c = common::fake_drive(&tmp, "C", "0003");
    for i in 0..3u64 {
        let body = common::noise(4 * KIB + i as usize, 100 + i);
        common::put(&a.mount_point, &format!("Docs/f{i}.bin"), &body);
        common::put(&b.mount_point, &format!("old backup/f{i}.bin"), &body);
    }
    common::put(&c.mount_point, "movie.bin", &common::noise(64 * KIB, 7));
    let ids = catalog_all(&mut store, &provider, &[&a, &b, &c]);
    let plan = recommend_plan(store.conn()).unwrap();
    assert_eq!(plan.recommended_backup.as_deref(), Some(ids[1].as_str()));
    let b_entry = plan.entries.iter().find(|e| e.volume_id == ids[1]).unwrap();
    assert_eq!(b_entry.proposed_role, DriveRole::Backup);
    assert!(b_entry.backup_copy_share > 0.4);
}

#[test]
fn apply_plan_adopts_existing_copies_into_aegis_backup() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let body = common::noise(6 * KIB, 110);
    common::put(&a.mount_point, "Photos/x.jpg", &body);
    let stray = common::put(&b.mount_point, "random/x.jpg", &body);
    let ids = catalog_all(&mut store, &provider, &[&a, &b]);

    let applied = apply_plan(
        &mut store,
        &[(ids[0].clone(), DriveRole::General), (ids[1].clone(), DriveRole::Backup)],
    )
    .unwrap();
    let batch = applied.adopt_batch.expect("adopt batch");
    run_batch(&mut store, &batch);

    let adopted = b.mount_point.join("Aegis Backup").join("A").join("Photos").join("x.jpg");
    assert!(adopted.is_file(), "moved within the Backup drive");
    assert!(!stray.exists());
    assert_eq!(count(&store, "SELECT COUNT(*) FROM backup_copies"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM backup_sources WHERE removed_at IS NULL"), 1);
}

#[test]
fn only_copy_on_backup_drive_is_flagged_not_moved() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    common::put(&a.mount_point, "a.txt", b"only on a");
    let unique = common::put(&b.mount_point, "notes/unique.txt", b"only on the backup drive");
    let ids = catalog_all(&mut store, &provider, &[&a, &b]);
    let applied = apply_plan(
        &mut store,
        &[(ids[0].clone(), DriveRole::General), (ids[1].clone(), DriveRole::Backup)],
    )
    .unwrap();
    assert_eq!(applied.only_copies_on_backup, 1);
    assert!(applied.adopt_batch.is_none());
    assert!(unique.is_file());
}

#[test]
fn stray_backup_moves_via_copy_then_quarantine() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let c = common::fake_drive(&tmp, "C", "0003");
    let body = common::noise(6 * KIB, 120);
    let pa = common::put(&a.mount_point, "Work/y.bin", &body);
    let pc = common::put(&c.mount_point, "copies/y.bin", &body);
    set_mtime(&pc, 10);
    set_mtime(&pa, 1000); // A's copy is the original
    let ids = catalog_all(&mut store, &provider, &[&a, &b, &c]);
    let hash = blake3::hash(&body).as_bytes().to_vec();
    let before = aegis_core::dupes::copy_count(store.conn(), &hash).unwrap();

    let applied = apply_plan(
        &mut store,
        &[
            (ids[0].clone(), DriveRole::General),
            (ids[1].clone(), DriveRole::Backup),
            (ids[2].clone(), DriveRole::General),
        ],
    )
    .unwrap();
    let batch = applied.stray_batch.expect("stray batch");
    run_batch(&mut store, &batch);

    let on_backup = b.mount_point.join("Aegis Backup").join("A").join("Work").join("y.bin");
    assert!(on_backup.is_file(), "backup copy written");
    assert!(pa.is_file(), "the original stays");
    assert!(!pc.exists(), "the stray copy is in quarantine");
    assert_eq!(aegis_core::dupes::copy_count(store.conn(), &hash).unwrap(), before);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM quarantine_entries"), 1);
}

#[test]
fn second_backup_role_rejected() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let ids = catalog_all(&mut store, &provider, &[&a, &b]);
    let err = apply_plan(
        &mut store,
        &[(ids[0].clone(), DriveRole::Backup), (ids[1].clone(), DriveRole::Backup)],
    )
    .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("one backup drive"), "{err}");
}

#[test]
fn make_this_the_backup_copy_moves_instead_of_deleting() {
    use aegis_core::dupes::decisions::{DupeChoice, decide};
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let b = common::fake_drive(&tmp, "B", "0002");
    let c = common::fake_drive(&tmp, "C", "0003");
    let body = common::noise(6 * KIB, 130);
    common::put(&a.mount_point, "Docs/z.bin", &body);
    let pc = common::put(&c.mount_point, "Docs/z.bin", &body);
    let ids = catalog_all(&mut store, &provider, &[&a, &b, &c]);
    store
        .conn()
        .execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&ids[1]])
        .unwrap();
    let g = aegis_core::dupes::grouped_report(store.conn()).unwrap().groups.remove(0);
    let victim = g.locations.iter().position(|l| l.volume_id == ids[2]).unwrap();
    let out = decide(&mut store, &g.group_id, DupeChoice::MakeThisTheBackupCopy { location_index: victim }).unwrap();
    run_batch(&mut store, &out.batch_id.unwrap());
    assert!(!pc.exists());
    assert!(b.mount_point.join("Aegis Backup").join("A").join("Docs").join("z.bin").is_file());
}

fn set_mtime(path: &Path, secs_ago: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago))
        .unwrap();
}
