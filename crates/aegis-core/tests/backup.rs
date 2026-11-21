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

// ---------------------------------------------------------------- Task 10: top-up

use aegis_core::backup::topup::{
    TopUpAuthority, TopUpKind, list_previous_versions, plan_topup, prune_previous_versions,
    restore_previous_version, run_topup,
};
use aegis_core::scanner::scan_root;
use std::path::PathBuf;

/// A PC folder named "Documents" (a configured root) plus a Backup drive "B".
struct BackupFx {
    tmp: tempfile::TempDir,
    store: SqliteStore,
    provider: FakeVolumeProvider,
    docs: PathBuf,
    backup: MountedVolume,
    backup_id: String,
}

fn backup_fx(files: &[(&str, &[u8])]) -> BackupFx {
    let (tmp, mut store, provider) = setup();
    let docs = tmp.path().join("Documents");
    for (rel, body) in files {
        common::put(&docs, rel, body);
    }
    store.add_root(&docs).unwrap();
    scan_root(&mut store, &docs, &[1]).unwrap();
    let backup = common::fake_drive(&tmp, "B", "0002");
    let backup_id = common::plug_and_catalog(&mut store, &provider, &backup);
    store
        .conn()
        .execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&backup_id])
        .unwrap();
    add_source_for(&store, &docs);
    BackupFx { tmp, store, provider, docs, backup, backup_id }
}

/// Registers `folder` (on whatever volume holds it) as a backup source.
fn add_source_for(store: &SqliteStore, folder: &Path) {
    let canon = common::canon(folder);
    let (vid, mount) = store
        .list_volumes()
        .unwrap()
        .into_iter()
        .filter(|v| v.status == "online" && aegis_core::db::files::path_is_within(&canon, &v.mount_point, false))
        .max_by_key(|v| v.mount_point.len())
        .map(|v| (v.id, v.mount_point))
        .unwrap();
    let rel = canon[mount.len()..].trim_start_matches(['\\', '/']).to_string();
    store
        .conn()
        .execute(
            "INSERT INTO backup_sources (id, volume_id, rel_path, tier, pinned, added_at) VALUES (?1, ?2, ?3, 2, 0, 0)",
            rusqlite::params![format!("src-{rel}"), vid, rel],
        )
        .unwrap();
}

fn topup(fx: &mut BackupFx) -> aegis_core::backup::topup::TopUpResult {
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap()
}

fn live_copy(fx: &BackupFx, rel: &str) -> PathBuf {
    let mut p = fx.backup.mount_point.join("Aegis Backup").join("PC (Documents)");
    for part in rel.split('/') {
        p = p.join(part);
    }
    p
}

#[test]
fn topup_adds_new_files_as_plain_copies() {
    let mut fx = backup_fx(&[("a.txt", b"alpha"), ("sub/b.txt", b"beta")]);
    let res = topup(&mut fx);
    assert!(res.completed);
    assert_eq!(res.added, 2);
    assert_eq!(std::fs::read(live_copy(&fx, "a.txt")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(live_copy(&fx, "sub/b.txt")).unwrap(), b"beta");
    assert_eq!(count(&fx.store, "SELECT COUNT(*) FROM backup_copies"), 2);
    // A second run has nothing to do.
    assert!(plan_topup(fx.store.conn(), &fx.backup_id).unwrap().items.is_empty());
}

#[test]
fn changed_source_moves_old_copy_to_previous_versions() {
    let files: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.txt"), format!("v1 {i}").into_bytes())).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let mut fx = backup_fx(&refs);
    topup(&mut fx);

    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(fx.docs.join("f0.txt"), b"v2 changed").unwrap();
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].kind, TopUpKind::Replace);
    let res = run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(res.replaced, 1);

    assert_eq!(std::fs::read(live_copy(&fx, "f0.txt")).unwrap(), b"v2 changed");
    let today = aegis_core::format_date_ms(aegis_core::now_ms());
    let old = fx
        .backup
        .mount_point
        .join("Aegis Backup")
        .join("_Previous versions")
        .join(&today)
        .join("PC (Documents)")
        .join("f0.txt");
    assert_eq!(std::fs::read(&old).unwrap(), b"v1 0");
    let versions = list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].reason, "changed");
}

#[test]
fn deleted_source_retires_only_when_source_online() {
    let (tmp, mut store, provider) = setup();
    let s = common::fake_drive(&tmp, "S", "0005");
    for i in 0..6 {
        common::put(&s.mount_point, &format!("Keep/f{i}.txt"), format!("file {i}").as_bytes());
    }
    let b = common::fake_drive(&tmp, "B", "0002");
    let _s_id = common::plug_and_catalog(&mut store, &provider, &s);
    let b_id = common::plug_and_catalog(&mut store, &provider, &b);
    store.conn().execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&b_id]).unwrap();
    add_source_for(&store, &s.mount_point.join("Keep"));
    let plan = plan_topup(store.conn(), &b_id).unwrap();
    run_topup(&mut store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();

    std::fs::remove_file(s.mount_point.join("Keep").join("f0.txt")).unwrap();
    provider.unplug(&s.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let plan = plan_topup(store.conn(), &b_id).unwrap();
    assert!(
        plan.items.iter().all(|i| i.kind != TopUpKind::RetireDeleted),
        "an unplugged source is never a deletion"
    );

    provider.plug(s.clone());
    sync_mounted(&mut store, &provider).unwrap();
    let s_id = common::volume_id_at(&store, &s.mount_point);
    aegis_core::scanner::catalog_volume(&mut store, &s_id, &mut |_| {}).unwrap();
    let plan = plan_topup(store.conn(), &b_id).unwrap();
    assert_eq!(
        plan.items.iter().filter(|i| i.kind == TopUpKind::RetireDeleted).count(),
        1
    );
    let res = run_topup(&mut store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(res.retired, 1);
    assert!(!b.mount_point.join("Aegis Backup").join("S").join("Keep").join("f0.txt").exists());
}

#[test]
fn interrupted_copy_leaves_no_partial_at_live_path_and_resumes() {
    let big = common::noise(2 * 1024 * KIB, 140);
    let mut fx = backup_fx(&[("big.bin", &big)]);
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let cancel = AtomicBool::new(false);
    let res = run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &cancel, &mut |done, _| {
        if done > 0 {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(!res.completed);
    assert!(!live_copy(&fx, "big.bin").exists(), "no half-written file at the live path");

    let res = topup(&mut fx);
    assert!(res.completed);
    assert_eq!(std::fs::read(live_copy(&fx, "big.bin")).unwrap(), big);
    let partials = walk_count(&fx.backup.mount_point, ".aegis-partial");
    assert_eq!(partials, 0, "stale partial files are cleaned up");
}

#[test]
fn source_changed_during_copy_is_discarded() {
    let big = common::noise(1024 * KIB, 141);
    let mut fx = backup_fx(&[("big.bin", &big)]);
    let src = fx.docs.join("big.bin");
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let mut touched = false;
    let res = run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {
        if !touched {
            touched = true;
            set_mtime(&src, 5);
        }
    })
    .unwrap();
    assert_eq!(res.added, 0);
    assert_eq!(count(&fx.store, "SELECT COUNT(*) FROM backup_copies"), 0);
    assert!(!live_copy(&fx, "big.bin").exists());
}

#[test]
fn prune_removes_only_expired_versions() {
    let files: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.txt"), format!("v1 {i}").into_bytes())).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let mut fx = backup_fx(&refs);
    topup(&mut fx);
    for name in ["f0.txt", "f1.txt"] {
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(fx.docs.join(name), b"v2").unwrap();
        scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
        topup(&mut fx);
    }
    let versions = list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(versions.len(), 2);
    // Age the first version past the window.
    let ninety_days = 90 * 86_400_000i64;
    fx.store
        .conn()
        .execute(
            "UPDATE backup_versions SET moved_at = moved_at - ?2, prune_after = prune_after - ?2 WHERE id = ?1",
            rusqlite::params![versions[0].id, ninety_days + 1],
        )
        .unwrap();
    let pruned = prune_previous_versions(&mut fx.store, &fx.backup_id, aegis_core::now_ms()).unwrap();
    assert_eq!(pruned, 1);
    let left = list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, versions[1].id);
    assert_eq!(walk_count(&fx.backup.mount_point.join("Aegis Backup").join("_Previous versions"), ".txt"), 1);
}

#[test]
fn restore_previous_version_never_overwrites() {
    let files: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.txt"), format!("v1 {i}").into_bytes())).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let mut fx = backup_fx(&refs);
    topup(&mut fx);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(fx.docs.join("f0.txt"), b"v2").unwrap();
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
    topup(&mut fx);
    let v = list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap().remove(0);
    let restored = restore_previous_version(&mut fx.store, &v.id, None).unwrap();
    assert_eq!(restored.file_name().unwrap().to_string_lossy(), "f0 (restored).txt");
    assert_eq!(std::fs::read(&restored).unwrap(), b"v1 0");
    assert_eq!(std::fs::read(fx.docs.join("f0.txt")).unwrap(), b"v2", "current file untouched");
    let _ = &fx.tmp;
    let _ = &fx.provider;
}

fn walk_count(dir: &Path, suffix: &str) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                walk_count(&p, suffix)
            } else {
                usize::from(p.to_string_lossy().ends_with(suffix))
            }
        })
        .sum()
}

// ---------------------------------------------------------------- Task 11: standing approval

use aegis_core::policy::{grant_standing_topup, has_standing_topup};

#[test]
fn standing_requires_first_approved_topup() {
    let mut fx = backup_fx(&[("a.txt", b"alpha")]);
    assert!(grant_standing_topup(&mut fx.store, &fx.backup_id).is_err(), "no approved run yet");
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert!(
        run_topup(&mut fx.store, &plan, TopUpAuthority::Standing, &AtomicBool::new(false), &mut |_, _| {}).is_err(),
        "standing runs need a grant"
    );

    topup(&mut fx); // first run, approved by the owner
    grant_standing_topup(&mut fx.store, &fx.backup_id).unwrap();
    assert!(has_standing_topup(fx.store.conn(), &fx.backup_id).unwrap());

    std::fs::write(fx.docs.join("new.txt"), b"new").unwrap();
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let res = run_topup(&mut fx.store, &plan, TopUpAuthority::Standing, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(res.added, 1);
    assert_eq!(
        count(&fx.store, "SELECT COUNT(*) FROM audit_log WHERE action IN ('standing_approval.granted','backup.topup')"),
        3
    );
}

#[test]
fn standing_revoked_on_clone_and_role_change() {
    let mut fx = backup_fx(&[("a.txt", b"alpha")]);
    topup(&mut fx);
    grant_standing_topup(&mut fx.store, &fx.backup_id).unwrap();

    // A clone of the Backup drive appears.
    let clone = common::fake_drive(&fx.tmp, "B-clone", "0002");
    std::fs::create_dir_all(clone.mount_point.join(".aegis")).unwrap();
    std::fs::copy(
        fx.backup.mount_point.join(".aegis").join("volume.json"),
        clone.mount_point.join(".aegis").join("volume.json"),
    )
    .unwrap();
    fx.provider.plug(clone);
    sync_mounted(&mut fx.store, &fx.provider).unwrap();
    assert!(!has_standing_topup(fx.store.conn(), &fx.backup_id).unwrap(), "clone revokes");

    topup(&mut fx);
    grant_standing_topup(&mut fx.store, &fx.backup_id).unwrap();
    assert!(has_standing_topup(fx.store.conn(), &fx.backup_id).unwrap());
    apply_plan(&mut fx.store, &[(fx.backup_id.clone(), DriveRole::General)]).unwrap();
    assert!(!has_standing_topup(fx.store.conn(), &fx.backup_id).unwrap(), "role change revokes");
}

#[test]
fn standing_run_refuses_paths_outside_backup_root() {
    let mut fx = backup_fx(&[("a.txt", b"alpha")]);
    topup(&mut fx);
    grant_standing_topup(&mut fx.store, &fx.backup_id).unwrap();
    std::fs::write(fx.docs.join("b.txt"), b"beta").unwrap();
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
    let mut plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.items.len(), 1);
    plan.items[0].backup_rel_path = "Aegis Backup\\..\\..\\escaped.txt".into();
    let err = run_topup(&mut fx.store, &plan, TopUpAuthority::Standing, &AtomicBool::new(false), &mut |_, _| {})
        .unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
    assert!(!fx.tmp.path().join("escaped.txt").exists());
    assert!(!fx.backup.mount_point.join("escaped.txt").exists());
}
