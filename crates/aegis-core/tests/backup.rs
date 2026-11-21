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

// ---------------------------------------------------------------- Task 12: guard

use aegis_core::backup::guard::{looks_encrypted, override_halt};

/// Backs up `n_a` files under Documents\A and `n_b` under Documents\B as two sources.
fn two_source_fx(n_a: usize, n_b: usize, ext: &str) -> BackupFx {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..n_a {
        files.push((format!("A/f{i}.{ext}"), doc_bytes(i)));
    }
    for i in 0..n_b {
        files.push((format!("B/f{i}.{ext}"), doc_bytes(100 + i)));
    }
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let mut fx = backup_fx(&refs);
    fx.store.conn().execute("DELETE FROM backup_sources", []).unwrap();
    add_source_for(&fx.store, &fx.docs.join("A"));
    add_source_for(&fx.store, &fx.docs.join("B"));
    topup(&mut fx);
    fx
}

/// A plausible .docx: zip magic followed by ordinary text.
fn doc_bytes(i: usize) -> Vec<u8> {
    let mut v = b"PK\x03\x04".to_vec();
    v.extend_from_slice(format!("document number {i} with ordinary words in it. ").repeat(40).as_bytes());
    v
}

fn change(fx: &mut BackupFx, rels: &[String], random: bool) {
    std::thread::sleep(std::time::Duration::from_millis(20));
    for (k, rel) in rels.iter().enumerate() {
        let body = if random { common::noise(8 * KIB, 500 + k as u64) } else { format!("edited {k}").into_bytes() };
        std::fs::write(fx.docs.join(rel), body).unwrap();
    }
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
}

#[test]
fn twenty_one_percent_change_halts_folder_only() {
    let mut fx = two_source_fx(19, 19, "txt");
    let a: Vec<String> = (0..4).map(|i| format!("A/f{i}.txt")).collect(); // 4/19 = 21%
    let b: Vec<String> = vec!["B/f0.txt".into()];
    change(&mut fx, &[a, b].concat(), false);
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.halted_folders.len(), 1);
    let h = &plan.halted_folders[0];
    assert!(h.source_folder.ends_with('A'), "{h:?}");
    assert_eq!((h.changed, h.total), (4, 19));
    assert_eq!(plan.items.len(), 1, "B's change still goes ahead");
}

#[test]
fn twenty_percent_exactly_proceeds() {
    let mut fx = two_source_fx(10, 0, "txt");
    change(&mut fx, &["A/f0.txt".to_string(), "A/f1.txt".to_string()], false);
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert!(plan.halted_folders.is_empty());
    assert_eq!(plan.items.len(), 2);
}

#[test]
fn encrypted_looking_files_halt_below_mass_threshold() {
    let mut fx = two_source_fx(20, 0, "docx");
    change(&mut fx, &["A/f0.docx".to_string(), "A/f1.docx".to_string()], true); // 10%
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.halted_folders.len(), 1);
    assert!(plan.halted_folders[0].suspect_encrypted >= 2);
    assert!(plan.items.is_empty());
}

#[test]
fn real_jpgs_are_not_suspect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpg.extend(common::noise(64 * KIB, 9)); // compressed image data looks random
    let p = common::put(tmp.path(), "photo.jpg", &jpg);
    assert!(!looks_encrypted(&p, Some("jpg")).unwrap());
    let fake = common::put(tmp.path(), "photo2.jpg", &common::noise(64 * KIB, 10));
    assert!(looks_encrypted(&fake, Some("jpg")).unwrap(), "random bytes with a .jpg name");
    let txt = common::put(tmp.path(), "notes.txt", b"plain words, nothing odd here at all");
    assert!(!looks_encrypted(&txt, Some("txt")).unwrap());
}

#[test]
fn override_allows_next_run_once() {
    let mut fx = two_source_fx(10, 0, "txt");
    let first: Vec<String> = (0..5).map(|i| format!("A/f{i}.txt")).collect();
    change(&mut fx, &first, false);
    let halted = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(halted.halted_folders.len(), 1);
    override_halt(&mut fx.store, &fx.backup_id, &halted.halted_folders[0].source_folder).unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert!(plan.halted_folders.is_empty());
    let res = run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(res.replaced, 5);

    let again: Vec<String> = (5..10).map(|i| format!("A/f{i}.txt")).collect();
    change(&mut fx, &again, false);
    assert_eq!(plan_topup(fx.store.conn(), &fx.backup_id).unwrap().halted_folders.len(), 1, "one-shot");
}

// ---------------------------------------------------------------- Task 13: priority fill

use aegis_core::backup::priority::{add_source, coverage_summary, remove_source, tier_for_file};

fn fid(store: &SqliteStore, path: &Path) -> String {
    store
        .conn()
        .query_row(
            "SELECT id FROM files WHERE path = ?1 AND deleted_at IS NULL",
            [common::canon(path)],
            |r| r.get(0),
        )
        .unwrap()
}

fn set_backup_capacity(fx: &BackupFx, bytes: i64) {
    fx.store
        .conn()
        .execute("UPDATE volumes SET total_bytes = ?2 WHERE id = ?1", rusqlite::params![fx.backup_id, bytes])
        .unwrap();
}

#[test]
fn only_copies_are_tier_one() {
    let shared = common::noise(3 * KIB, 600);
    let mut fx = backup_fx(&[("unique.txt", b"only here"), ("shared.bin", &shared)]);
    common::put(&fx.backup.mount_point, "elsewhere/shared.bin", &shared);
    aegis_core::scanner::catalog_volume(&mut fx.store, &fx.backup_id, &mut |_| {}).unwrap();
    run_fingerprint(&mut fx.store, &AtomicBool::new(false)).unwrap();
    assert_eq!(tier_for_file(fx.store.conn(), &fid(&fx.store, &fx.docs.join("unique.txt"))).unwrap(), Some(1));
    assert_eq!(tier_for_file(fx.store.conn(), &fid(&fx.store, &fx.docs.join("shared.bin"))).unwrap(), Some(2));
}

#[test]
fn installers_and_caches_never_backed_up() {
    let mut fx = backup_fx(&[("notes.txt", b"keep me"), ("proj/node_modules/dep/index.js", b"module.exports = 1;"), ("proj/package.json", b"{}")]);
    let downloads = fx.tmp.path().join("Downloads");
    let setup_exe = common::put(&downloads, "setup.exe", b"MZ installer bytes");
    common::put(&downloads, "manual.pdf", b"%PDF-1.4 manual");
    fx.store.add_root(&downloads).unwrap();
    scan_root(&mut fx.store, &downloads, &[1]).unwrap();
    add_source_for(&fx.store, &downloads);
    assert_eq!(tier_for_file(fx.store.conn(), &fid(&fx.store, &setup_exe)).unwrap(), None);

    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let names: Vec<String> = plan
        .items
        .iter()
        .map(|i| i.backup_rel_path.rsplit(['\\', '/']).next().unwrap().to_string())
        .collect();
    assert!(names.contains(&"notes.txt".to_string()));
    assert!(names.contains(&"manual.pdf".to_string()));
    assert!(!names.contains(&"setup.exe".to_string()), "{names:?}");
    assert!(!names.contains(&"index.js".to_string()), "{names:?}");
}

#[test]
fn fills_by_tier_when_space_short() {
    let doc = common::noise(3 * KIB, 601);
    let song = common::noise(8 * KIB, 602);
    let mut fx = backup_fx(&[("letter.txt", &doc), ("Music/song.mp3", &song)]);
    set_backup_capacity(&fx, 10 * KIB as i64);
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let names: Vec<String> = plan
        .items
        .iter()
        .map(|i| i.backup_rel_path.rsplit(['\\', '/']).next().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["letter.txt".to_string()]);
    assert_eq!(plan.skipped_for_space.len(), 1);
    assert_eq!(plan.skipped_for_space[0].tier, 3);
    assert_eq!(plan.skipped_for_space[0].bytes_skipped, (8 * KIB) as i64);

    run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    let cov = coverage_summary(fx.store.conn()).unwrap().expect("a backup drive");
    let music = cov.tiers.iter().find(|t| t.tier == 3).unwrap();
    assert_eq!(music.unprotected_bytes, (8 * KIB) as i64);
    assert!(cov.suggestion.as_deref().unwrap_or("").contains("doesn't fit"), "{:?}", cov.suggestion);
}

#[test]
fn prunes_oldest_versions_before_skipping_live() {
    let files: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.bin"), common::noise(2 * KIB, 700 + i))).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let mut fx = backup_fx(&refs);
    topup(&mut fx); // 12 KiB live
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(fx.docs.join("f0.bin"), common::noise(2 * KIB, 800)).unwrap();
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
    topup(&mut fx); // old f0 becomes a 2 KiB previous version
    assert_eq!(list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap().len(), 1);

    // Room for exactly one more 2 KiB file only if the previous version goes.
    aegis_core::scanner::catalog_volume(&mut fx.store, &fx.backup_id, &mut |_| {}).unwrap();
    set_backup_capacity(&fx, (14 * KIB) as i64);
    common::put(&fx.docs, "new.bin", &common::noise(2 * KIB, 801));
    scan_root(&mut fx.store, &fx.docs, &[1]).unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.items.len(), 1, "the new file is planned, not skipped");
    assert!(plan.skipped_for_space.is_empty());
    assert!(plan.prune_bytes > 0);
    let res = run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(res.added, 1);
    assert!(list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap().is_empty(), "oldest version pruned");
}

#[test]
fn remove_source_asks_bytes_and_retires_as_uncovered() {
    let mut fx = backup_fx(&[("a.txt", b"alpha"), ("b.txt", b"beta!")]);
    topup(&mut fx);
    let (src, vol, rel): (String, String, String) = fx
        .store
        .conn()
        .query_row("SELECT id, volume_id, rel_path FROM backup_sources", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    let bytes = remove_source(&mut fx.store, &src).unwrap();
    assert_eq!(bytes, 10);
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.items.iter().filter(|i| i.kind == TopUpKind::RetireUncovered).count(), 2);
    run_topup(&mut fx.store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    let versions = list_previous_versions(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().all(|v| v.reason == "uncovered"));

    // Adding it back, pinned, makes everything in it tier 1.
    add_source(&mut fx.store, &vol, &rel, true).unwrap();
    assert_eq!(tier_for_file(fx.store.conn(), &fid(&fx.store, &fx.docs.join("a.txt"))).unwrap(), Some(1));
    assert_eq!(count(&fx.store, "SELECT COUNT(*) FROM backup_sources WHERE removed_at IS NULL"), 1);
}

// ---------------------------------------------------------------- Task 16: end to end

#[test]
fn end_to_end_three_drives() {
    use aegis_core::backup::plan::recommend_plan;
    use aegis_core::backup::priority::add_source_path;
    use aegis_core::dupes::decisions::{DupeChoice, decide};
    use aegis_core::dupes::{copy_count, grouped_report};
    use aegis_core::policy::grant_standing_topup;

    let (tmp, mut store, provider) = setup();
    let photo = common::noise(6 * KIB, 900);
    let budget = common::noise(5 * KIB, 901);
    let old_zip = common::noise(7 * KIB, 902);

    // The PC: a Documents folder with originals.
    let docs = tmp.path().join("Documents");
    let report = common::put(&docs, "report.docx", b"PK\x03\x04 quarterly report v1");
    let p_photo = common::put(&docs, "photo1.jpg", &photo);
    let p_budget = common::put(&docs, "budget.xlsx", &budget);
    for i in 0..10 {
        common::put(&docs, &format!("notes/n{i}.txt"), format!("note number {i}").as_bytes());
    }
    for p in [&p_photo, &p_budget] {
        set_mtime(p, 5000); // the PC holds the originals
    }

    // Drive A: an extra copy of the PC photo, plus an archive original.
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "Photos/photo1.jpg", &photo);
    common::put(&a.mount_point, "Photos/photo2.jpg", &common::noise(4 * KIB, 903));
    let a_zip = common::put(&a.mount_point, "Archive/old.zip", &old_zip);
    set_mtime(&a_zip, 5000);
    // Drive B: random backup copies.
    let b = common::fake_drive(&tmp, "B", "0002");
    common::put(&b.mount_point, "backup/old.zip", &old_zip);
    common::put(&b.mount_point, "backup/budget.xlsx", &budget);
    // Drive C: another budget copy and some music.
    let c = common::fake_drive(&tmp, "C", "0003");
    common::put(&c.mount_point, "misc/budget.xlsx", &budget);
    common::put(&c.mount_point, "music/song.mp3", &common::noise(9 * KIB, 904));

    store.add_root(&docs).unwrap();
    scan_root(&mut store, &docs, &[1]).unwrap();
    let ids = catalog_all(&mut store, &provider, &[&a, &b, &c]);
    let hashes: Vec<Vec<u8>> = {
        let mut stmt = store.conn().prepare("SELECT DISTINCT blake3 FROM files WHERE blake3 IS NOT NULL").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert!(hashes.len() >= 3);

    // 1. Drive plan: B already holds the extra copies.
    let plan = recommend_plan(store.conn()).unwrap();
    assert!(plan.ready);
    assert_eq!(plan.recommended_backup.as_deref(), Some(ids[1].as_str()));

    // 2. Apply it and approve both generated batches.
    let applied = apply_plan(
        &mut store,
        &[
            (ids[0].clone(), DriveRole::General),
            (ids[1].clone(), DriveRole::Backup),
            (ids[2].clone(), DriveRole::General),
        ],
    )
    .unwrap();
    for batch in [applied.adopt_batch.clone(), applied.stray_batch.clone()].into_iter().flatten() {
        run_batch(&mut store, &batch);
    }
    assert!(b.mount_point.join("Aegis Backup").join("A").join("Archive").join("old.zip").is_file());

    // 3. Answer every remaining duplicate question.
    for g in grouped_report(store.conn()).unwrap().groups.into_iter().filter(|g| g.needs_question) {
        decide(&mut store, &g.group_id, DupeChoice::KeepBoth).unwrap();
    }
    assert!(grouped_report(store.conn()).unwrap().groups.iter().all(|g| !g.needs_question));

    // 4. Back up Documents: first run approved by the owner, then automatic.
    add_source_path(&mut store, &docs, false).unwrap();
    let plan = plan_topup(store.conn(), &ids[1]).unwrap();
    assert!(plan.halted_folders.is_empty());
    let first = run_topup(&mut store, &plan, TopUpAuthority::UserApproved, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert!(first.completed && first.added > 0);
    grant_standing_topup(&mut store, &ids[1]).unwrap();

    // 5. The owner edits one file and deletes another; the standing top-up follows.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&report, b"PK\x03\x04 quarterly report v2, edited").unwrap();
    std::fs::remove_file(docs.join("notes").join("n0.txt")).unwrap();
    scan_root(&mut store, &docs, &[1]).unwrap();
    let plan = plan_topup(store.conn(), &ids[1]).unwrap();
    assert!(plan.halted_folders.is_empty(), "{:?}", plan.halted_folders);
    let auto = run_topup(&mut store, &plan, TopUpAuthority::Standing, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!((auto.replaced, auto.retired), (1, 1));

    // S1: nothing that existed was ever left without a copy.
    for h in &hashes {
        let n = copy_count(store.conn(), h).unwrap();
        let hex: String = h.iter().map(|x| format!("{x:02x}")).collect();
        let known = [
            ("report v1", blake3::hash(b"PK\x03\x04 quarterly report v1")),
            ("note n0", blake3::hash(b"note number 0")),
        ];
        // The owner's own edit and delete are not Aegis removing a copy; they are kept as
        // previous versions, checked below.
        if known.iter().any(|(_, k)| k.as_bytes().as_slice() == h.as_slice()) {
            continue;
        }
        assert!(n >= 1, "Aegis left a file without any copy: {hex}");
    }
    // The old report and the deleted note wait in previous versions.
    let versions = list_previous_versions(store.conn(), &ids[1]).unwrap();
    assert!(versions.iter().any(|v| v.reason == "changed" && v.original_rel_path.ends_with("report.docx")));
    assert!(versions.iter().any(|v| v.reason == "deleted" && v.original_rel_path.ends_with("n0.txt")));
    // Everything in the backed-up folders is protected.
    let cov = coverage_summary(store.conn()).unwrap().unwrap();
    for t in cov.tiers.iter().filter(|t| t.tier != 0) {
        assert_eq!(t.unprotected_bytes, 0, "tier {} not fully protected: {cov:?}", t.tier);
    }
    // The audit trail holds the grant and both runs.
    assert_eq!(count(&store, "SELECT COUNT(*) FROM audit_log WHERE action = 'standing_approval.granted'"), 1);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM audit_log WHERE action = 'backup.topup' AND detail_json LIKE '%\"standing\"%'"),
        1
    );
}

// ---------------------------------------------------------------- final review fixes

/// Review I2: stray copies only ever come from General drives; a newer copy in a PC folder is
/// the owner's working file and is never moved or quarantined by the drive plan.
#[test]
fn stray_copies_never_come_from_the_pc() {
    let (tmp, mut store, provider) = setup();
    let docs = tmp.path().join("Documents");
    let body = common::noise(6 * KIB, 950);
    let pc = common::put(&docs, "report.pdf", &body);
    store.add_root(&docs).unwrap();
    scan_root(&mut store, &docs, &[1]).unwrap();
    let a = common::fake_drive(&tmp, "A", "0001");
    let old = common::put(&a.mount_point, "old/report.pdf", &body);
    set_mtime(&old, 5000); // the drive copy is older, so the PC copy looks like the "extra"
    let b = common::fake_drive(&tmp, "B", "0002");
    let ids = catalog_all(&mut store, &provider, &[&a, &b]);
    let applied = apply_plan(
        &mut store,
        &[(ids[0].clone(), DriveRole::General), (ids[1].clone(), DriveRole::Backup)],
    )
    .unwrap();
    if let Some(batch) = applied.stray_batch {
        let touches_pc: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM actions WHERE batch_id = ?1 AND action_type = 'quarantine' AND from_path = ?2",
                rusqlite::params![batch, common::canon(&pc)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(touches_pc, 0, "the PC copy must not be quarantined");
    }
}

/// Review I5: a top-up first refreshes its source folders, so edits and new files made since
/// the last scan are backed up instead of being reported as "up to date".
#[test]
fn topup_sees_changes_made_since_the_last_scan() {
    use aegis_core::backup::topup::refresh_sources;
    let files: Vec<(String, Vec<u8>)> = (0..8).map(|i| (format!("f{i}.txt"), format!("v1 {i}").into_bytes())).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let mut fx = backup_fx(&refs);
    topup(&mut fx);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(fx.docs.join("f0.txt"), b"edited without a rescan").unwrap();
    std::fs::write(fx.docs.join("brand-new.txt"), b"new").unwrap();
    refresh_sources(&mut fx.store).unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let kinds: Vec<TopUpKind> = plan.items.iter().map(|i| i.kind).collect();
    assert!(kinds.contains(&TopUpKind::Replace), "{kinds:?}");
    assert!(kinds.contains(&TopUpKind::Add), "{kinds:?}");
}

// ---------------------------------------------------------------- engine: tripwire lock

use aegis_core::backup::guard::{lock_folder, locked_folders, release_lock};

#[test]
fn locked_folder_halts_topup() {
    let mut fx = two_source_fx(10, 10, "txt");
    change(&mut fx, &["A/f0.txt".to_string(), "B/f0.txt".to_string()], false); // 10%: below mass
    let docs = common::canon(&fx.docs);
    lock_folder(&mut fx.store, &format!("{docs}/A"), "tripwire").unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert_eq!(plan.halted_folders.len(), 1);
    assert!(plan.halted_folders[0].source_folder.ends_with('A'));
    assert_eq!(plan.items.len(), 1, "B is not locked");
}

#[test]
fn owner_override_unlocks_tripped_folder() {
    let mut fx = two_source_fx(10, 0, "txt");
    change(&mut fx, &["A/f0.txt".to_string()], false);
    let docs = common::canon(&fx.docs);
    // The tripwire locks the whole PC root; the Drives card overrides the source folder inside it.
    lock_folder(&mut fx.store, &docs, "tripwire").unwrap();
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    let folder = plan.halted_folders[0].source_folder.clone();
    override_halt(&mut fx.store, &fx.backup_id, &folder).unwrap();
    assert!(locked_folders(fx.store.conn()).unwrap().is_empty());
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert!(plan.halted_folders.is_empty());
    assert_eq!(plan.items.len(), 1);
}

#[test]
fn it_was_me_releases_lock_and_skips_mass_guard_once() {
    let mut fx = two_source_fx(10, 0, "txt");
    let all: Vec<String> = (0..5).map(|i| format!("A/f{i}.txt")).collect(); // 50%: mass change
    change(&mut fx, &all, false);
    let docs = common::canon(&fx.docs);
    lock_folder(&mut fx.store, &docs, "tripwire").unwrap();
    release_lock(&mut fx.store, Some(&fx.backup_id), &docs).unwrap();
    assert!(locked_folders(fx.store.conn()).unwrap().is_empty());
    let plan = plan_topup(fx.store.conn(), &fx.backup_id).unwrap();
    assert!(plan.halted_folders.is_empty(), "{:?}", plan.halted_folders);
    assert_eq!(plan.items.len(), 5);
}
