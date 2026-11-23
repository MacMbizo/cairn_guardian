//! Always-on engine: scheduling, reminders, watches, tripwire, janitor, space and the weekly note.

mod common;

use aegis_core::db::SqliteStore;
use aegis_core::engine::{record_run, watch_state};

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().unwrap()
}

#[test]
fn record_run_tracks_last_ok_only_on_success() {
    let s = store();
    let c = s.conn();
    record_run(c, "w", 100, None).unwrap();
    assert_eq!(watch_state(c, "w").unwrap().last_ok_at, Some(100));

    record_run(c, "w", 200, Some("boom")).unwrap();
    let st = watch_state(c, "w").unwrap();
    assert_eq!(st.last_run_at, Some(200));
    assert_eq!(st.last_ok_at, Some(100));
    assert_eq!(st.last_error.as_deref(), Some("boom"));

    assert_eq!(watch_state(c, "missing").unwrap().last_run_at, None);
}

// ---------- Task 2: calm gating and scheduler ----------

use aegis_core::engine::{Calm, Engine, Finding, Watch, is_due};

#[test]
fn calm_requires_idle_power_and_not_busy() {
    let calm = Calm { idle_secs: 120, on_ac: true, battery_pct: None, busy: false };
    assert!(calm.is_calm());
    assert!(!Calm { idle_secs: 119, ..calm }.is_calm());
    assert!(!Calm { on_ac: false, battery_pct: Some(49), ..calm }.is_calm());
    assert!(Calm { on_ac: false, battery_pct: Some(50), ..calm }.is_calm());
    assert!(!Calm { busy: true, ..calm }.is_calm());
}

#[test]
fn idle_watch_runs_anyway_after_grace() {
    assert!(!is_due(1000, Some(0), 1000, true, false));
    assert!(is_due(1000, Some(0), 1000 + 86_400_000, true, false));
    assert!(is_due(1000, Some(0), 1000, true, true));
    assert!(!is_due(1000, Some(0), 999, false, true));
    // Never run: light watches start at once; heavy ones wait for a calm moment (spec §7).
    assert!(is_due(1000, None, 0, false, false));
    assert!(!is_due(1000, None, 0, true, false));
    assert!(is_due(1000, None, 0, true, true));
}

#[test]
fn first_heavy_run_waits_for_calm_then_grace() {
    let mut s = store();
    let busy = Calm { idle_secs: 5, on_ac: true, battery_pct: None, busy: false };
    let mut engine = Engine::new(vec![Box::new(FakeWatch { name: "heavy", idle: true, fail: false })]);
    assert!(engine.tick(&mut s, 1_000, &busy, false).unwrap().ran.is_empty());
    assert!(engine.tick(&mut s, 1_000 + 3_600_000, &busy, false).unwrap().ran.is_empty());
    // A day after the engine first saw it, it runs anyway.
    assert_eq!(engine.tick(&mut s, 1_000 + 86_400_000, &busy, false).unwrap().ran, vec!["heavy"]);
}

struct FakeWatch {
    name: &'static str,
    idle: bool,
    fail: bool,
}

impl Watch for FakeWatch {
    fn name(&self) -> &'static str {
        self.name
    }
    fn every_ms(&self) -> i64 {
        1000
    }
    fn needs_idle(&self) -> bool {
        self.idle
    }
    fn kinds(&self) -> &'static [&'static str] {
        &[]
    }
    fn run(&mut self, _store: &mut SqliteStore, _now: i64) -> aegis_core::error::Result<Vec<Finding>> {
        if self.fail {
            Err(aegis_core::error::AegisError::InvalidInput("boom".into()))
        } else {
            Ok(vec![])
        }
    }
}

const CALM: Calm = Calm { idle_secs: 600, on_ac: true, battery_pct: None, busy: false };

#[test]
fn failing_watch_does_not_stop_others() {
    let mut s = store();
    let mut engine = Engine::new(vec![
        Box::new(FakeWatch { name: "bad", idle: false, fail: true }),
        Box::new(FakeWatch { name: "good", idle: false, fail: false }),
    ]);
    let report = engine.tick(&mut s, 5_000, &CALM, false).unwrap();
    assert_eq!(report.ran.len(), 2);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(watch_state(s.conn(), "good").unwrap().last_ok_at, Some(5_000));
    assert_eq!(watch_state(s.conn(), "bad").unwrap().last_ok_at, None);
}

#[test]
fn paused_skips_idle_watches_only() {
    let mut s = store();
    let mut engine = Engine::new(vec![
        Box::new(FakeWatch { name: "idle", idle: true, fail: false }),
        Box::new(FakeWatch { name: "event", idle: false, fail: false }),
    ]);
    let report = engine.tick(&mut s, 5_000, &CALM, true).unwrap();
    assert_eq!(report.ran, vec!["event"]);
}

// ---------- Task 3: reminders ----------

use aegis_core::engine::reminders::{
    TrayState, act, dismiss, list_open, mark_shown, sync_findings, to_deliver, tray_state, undemote,
};
use aegis_core::engine::{FindingAction, Level, Screen};

const DAY: i64 = 86_400_000;

fn finding(kind: &str, subject: &str, level: Level, title: &str) -> Finding {
    Finding {
        kind: kind.into(),
        subject: subject.into(),
        level,
        title: title.into(),
        detail: "d".into(),
        bytes: None,
        action: FindingAction::Open { screen: Screen::Drives },
    }
}

fn all_rows(c: &rusqlite::Connection) -> Vec<(String, String, String, String)> {
    let mut st = c.prepare("SELECT subject, level, state, title FROM reminders ORDER BY subject").unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn raising_same_subject_twice_updates_one_row() {
    let s = store();
    let c = s.conn();
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "first")], 1).unwrap();
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "second")], 2).unwrap();
    let rows = all_rows(c);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].3, "second");
}

#[test]
fn missing_subject_resolves() {
    let s = store();
    let c = s.conn();
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "t")], 1).unwrap();
    sync_findings(c, &["k"], &[], 2).unwrap();
    assert_eq!(all_rows(c)[0].2, "resolved");
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "t")], 3).unwrap();
    assert_eq!(all_rows(c)[0].2, "open");
}

#[test]
fn one_convenient_per_day() {
    let s = store();
    let c = s.conn();
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "a"), finding("k", "b", Level::Convenient, "b")], 0)
        .unwrap();
    let t = 1_000;
    let first = to_deliver(c, t, false, false).unwrap();
    assert_eq!(first.len(), 1);
    mark_shown(c, &first[0].id, t).unwrap();
    assert!(to_deliver(c, t + 3_600_000, false, false).unwrap().is_empty());
    let next = to_deliver(c, t + DAY + 1, false, false).unwrap();
    assert_eq!(next.len(), 1);
    assert_ne!(next[0].id, first[0].id);
}

#[test]
fn busy_holds_convenient_not_now() {
    let s = store();
    let c = s.conn();
    sync_findings(c, &["n"], &[finding("n", "x", Level::Now, "urgent")], 0).unwrap();
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "later")], 0).unwrap();
    let out = to_deliver(c, 10, true, false).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].level, Level::Now);
    let paused = to_deliver(c, 10, false, true).unwrap();
    assert_eq!(paused.len(), 1);
    assert_eq!(paused[0].level, Level::Now);
}

#[test]
fn three_dismissals_demote_convenient_kind() {
    let s = store();
    let c = s.conn();
    let f = [finding("k", "a", Level::Convenient, "t")];
    sync_findings(c, &["k"], &f, 0).unwrap();
    let id = list_open(c).unwrap()[0].id.clone();
    assert!(!dismiss(c, &id, 1).unwrap());
    assert!(!dismiss(c, &id, 2).unwrap());
    assert!(dismiss(c, &id, 3).unwrap());
    sync_findings(c, &["k"], &f, 4).unwrap();
    assert_eq!(list_open(c).unwrap()[0].level, Level::Note);
    undemote(c, "k").unwrap();
    sync_findings(c, &["k"], &f, 5).unwrap();
    assert_eq!(list_open(c).unwrap()[0].level, Level::Convenient);
}

#[test]
fn now_kind_never_demoted() {
    let s = store();
    let c = s.conn();
    let f = [finding("n", "x", Level::Now, "t")];
    sync_findings(c, &["n"], &f, 0).unwrap();
    let id = list_open(c).unwrap()[0].id.clone();
    for t in 1..=5 {
        assert!(!dismiss(c, &id, t).unwrap());
    }
    sync_findings(c, &["n"], &f, 6).unwrap();
    assert_eq!(list_open(c).unwrap()[0].level, Level::Now);
}

#[test]
fn acting_resets_streak() {
    let s = store();
    let c = s.conn();
    let f = [finding("k", "a", Level::Convenient, "t")];
    sync_findings(c, &["k"], &f, 0).unwrap();
    let id = list_open(c).unwrap()[0].id.clone();
    dismiss(c, &id, 1).unwrap();
    dismiss(c, &id, 2).unwrap();
    act(c, &id, 3).unwrap();
    sync_findings(c, &["k"], &f, 4).unwrap();
    let id = list_open(c).unwrap()[0].id.clone();
    assert!(!dismiss(c, &id, 5).unwrap());
    sync_findings(c, &["k"], &f, 6).unwrap();
    assert_eq!(list_open(c).unwrap()[0].level, Level::Convenient);
}

#[test]
fn tray_state_reflects_worst_open_level() {
    let s = store();
    let c = s.conn();
    assert_eq!(tray_state(c).unwrap(), TrayState::Calm);
    sync_findings(c, &["j"], &[finding("j", "a", Level::Note, "t")], 0).unwrap();
    assert_eq!(tray_state(c).unwrap(), TrayState::Waiting);
    sync_findings(c, &["n"], &[finding("n", "x", Level::Now, "t")], 0).unwrap();
    assert_eq!(tray_state(c).unwrap(), TrayState::Urgent);
}

// ---------- Task 4: tripwire ----------

use aegis_core::engine::tripwire::{ChangeKind, Tripwire};
use aegis_core::scanner::scan_root;
use std::path::{Path, PathBuf};

/// A PC root with `n` indexed text files.
fn indexed_root(n: usize) -> (tempfile::TempDir, SqliteStore, PathBuf, String) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("Documents");
    for i in 0..n {
        common::put(&root, &format!("f{i}.txt"), format!("ordinary words in file {i}. ").repeat(20).as_bytes());
    }
    let mut s = store();
    s.add_root(&root).unwrap();
    scan_root(&mut s, &root, &[1]).unwrap();
    let canon = common::canon(&root);
    (tmp, s, root, canon)
}

fn file(root: &Path, i: usize) -> PathBuf {
    PathBuf::from(common::canon(&root.join(format!("f{i}.txt"))))
}

#[test]
fn burst_of_random_rewrites_trips() {
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon.clone()]);
    for i in 0..250 {
        std::fs::write(root.join(format!("f{i}.txt")), common::noise(8192, i as u64)).unwrap();
        tw.observe(&file(&root, i), ChangeKind::Modified, 1_000 + i as i64 * 40);
    }
    let trips = tw.evaluate(s.conn(), 11_000).unwrap();
    assert_eq!(trips.len(), 1);
    assert_eq!(trips[0].root, canon);
    assert_eq!(trips[0].changed, 250);
    assert!(trips[0].suspect_encrypted >= 1);
    assert!(tw.evaluate(s.conn(), 12_000).unwrap().is_empty(), "reported once per burst");
}

#[test]
fn burst_of_normal_edits_does_not_trip() {
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..250 {
        std::fs::write(root.join(format!("f{i}.txt")), format!("edited {i}")).unwrap();
        tw.observe(&file(&root, i), ChangeKind::Modified, 1_000);
    }
    assert!(tw.evaluate(s.conn(), 2_000).unwrap().is_empty());
}

#[test]
fn small_folder_change_below_fraction_does_not_trip() {
    let (_t, s, root, canon) = indexed_root(1_500);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..210 {
        std::fs::write(root.join(format!("f{i}.txt")), common::noise(8192, i as u64)).unwrap();
        tw.observe(&file(&root, i), ChangeKind::Modified, 1_000);
    }
    assert!(tw.evaluate(s.conn(), 2_000).unwrap().is_empty());
}

#[test]
fn mass_rename_to_new_extension_trips() {
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..250 {
        let to = root.join(format!("f{i}.txt.locked"));
        tw.observe(&file(&root, i), ChangeKind::Renamed { to }, 1_000);
    }
    let trips = tw.evaluate(s.conn(), 2_000).unwrap();
    assert_eq!(trips.len(), 1);
    assert_eq!(trips[0].new_ext_renames, 250);
}

#[test]
fn old_events_fall_out_of_window() {
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..250 {
        std::fs::write(root.join(format!("f{i}.txt")), common::noise(8192, i as u64)).unwrap();
        tw.observe(&file(&root, i), ChangeKind::Modified, 1_000);
    }
    assert!(tw.evaluate(s.conn(), 1_000 + 61_000).unwrap().is_empty());
}

// ---------- Task 5: live-index batching, interrupted jobs ----------

use aegis_core::drives::FakeVolumeProvider;
use aegis_core::engine::batcher::{ChangeBatcher, index_changed};
use aegis_core::scanner::mark_interrupted_jobs;

#[test]
fn batcher_waits_for_quiet_then_chunks() {
    let mut b = ChangeBatcher::new();
    for i in 0..1_200 {
        b.push(PathBuf::from(format!("C:/x/f{i}.txt")), i as i64 / 12);
    }
    for i in 0..100 {
        b.push(PathBuf::from(format!("C:/x/f{i}.txt")), 100);
    }
    assert!(b.drain_ready(1_500).is_empty());
    let chunks = b.drain_ready(2_100);
    assert_eq!(chunks.iter().map(|c| c.len()).collect::<Vec<_>>(), vec![500, 500, 200]);
    assert!(b.drain_ready(5_000).is_empty());
}

#[test]
fn index_changed_applies_all_chunks() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let drive = common::fake_drive(&tmp, "A", "0001");
    let id = common::plug_and_catalog(&mut s, &provider, &drive);
    let mut b = ChangeBatcher::new();
    for i in 0..600 {
        let p = common::put(&drive.mount_point, &format!("new/f{i}.txt"), b"x");
        b.push(PathBuf::from(common::canon(&p)), 0);
    }
    let n = index_changed(&mut s, &id, b.drain_ready(10_000)).unwrap();
    assert_eq!(n, 600);
    assert_eq!(common::live_file_paths(&s, &id).iter().filter(|p| p.contains("new")).count(), 600);
}

#[test]
fn stale_running_jobs_marked_interrupted() {
    let s = store();
    let c = s.conn();
    c.execute(
        "INSERT INTO scan_jobs (id, root_path, tier, status, started_at) VALUES ('j1', 'D:/', 1, 'running', 5)",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO scan_jobs (id, root_path, tier, status, started_at, finished_at) VALUES ('j2', 'D:/', 1, 'completed', 5, 6)",
        [],
    )
    .unwrap();
    assert_eq!(mark_interrupted_jobs(c, 99).unwrap(), 1);
    let st: (String, i64) =
        c.query_row("SELECT status, finished_at FROM scan_jobs WHERE id = 'j1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(st, ("interrupted".to_string(), 99));
}

// ---------- Task 6: safety watches ----------

use aegis_core::contracts::{QuarantineEntry, TargetKind};
use aegis_core::engine::watches::{
    DriveErrorsWatch, OnlyCopyWatch, SelfCheckWatch, catalogs_to_resume, only_copy_finding, quarantine_expiry_finding,
};

const GB: i64 = 1_000_000_000;

#[test]
fn only_copy_raises_after_seven_days_with_a_gigabyte() {
    assert!(only_copy_finding(GB, 6, "Seagate B").is_none());
    let f = only_copy_finding(GB, 7, "Seagate B").unwrap();
    assert_eq!(f.level, Level::Convenient);
    assert_eq!(f.subject, "backup");
    assert!(f.title.ends_with("exist only on this PC"), "{}", f.title);
    assert_eq!(f.detail, "Your Backup drive Seagate B hasn't been plugged in for 7 days.");
    assert!(only_copy_finding(GB - 1, 13, "B").is_none());
}

#[test]
fn only_copy_raises_any_amount_after_fourteen_days() {
    assert!(only_copy_finding(1, 14, "B").is_some());
    assert!(only_copy_finding(0, 400, "B").is_none());
}

#[test]
fn only_copy_silent_without_backup_drive() {
    let mut s = store();
    assert!(OnlyCopyWatch.run(&mut s, DAY * 100).unwrap().is_empty());
}

fn q_entry(id: &str, expires_at: i64) -> QuarantineEntry {
    QuarantineEntry {
        id: id.into(),
        action_id: "a".into(),
        target_kind: TargetKind::File,
        file_id: None,
        atomic_unit_id: None,
        volume_id: "v".into(),
        original_path: "C:/x".into(),
        quarantine_path: "C:/q".into(),
        size_bytes: 10,
        quarantined_at: 0,
        expires_at,
        restored_at: None,
        purged_at: None,
    }
}

#[test]
fn quarantine_expiry_counts_next_two_days_only() {
    let now = 10 * DAY;
    let entries = vec![
        q_entry("past", now - 1),
        q_entry("soon", now + DAY / 2),
        q_entry("edge", now + 2 * DAY),
        q_entry("later", now + 2 * DAY + 1),
    ];
    let f = quarantine_expiry_finding(&entries, now).unwrap();
    assert_eq!(f.title, "2 files are deleted for good tomorrow");
    assert_eq!(f.detail, "Keep any? They're in Disk Rescue & Quarantine.");
    assert_eq!(f.action, FindingAction::Open { screen: Screen::Rescue });
    assert!(quarantine_expiry_finding(&entries[3..], now).is_none());
    assert_eq!(quarantine_expiry_finding(&[q_entry("x", now + DAY / 4)], now).unwrap().title, "1 file is deleted for good today");
}

#[test]
fn drive_errors_raise_now_at_ten() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let drive = common::fake_drive(&tmp, "WD C", "0003");
    let id = common::plug_and_catalog(&mut s, &provider, &drive);
    let root = common::canon(&drive.mount_point);
    let errors = |n: usize| serde_json::to_string(&vec![serde_json::json!({"path": "x", "code": "io_error", "message": "read failed"}); n]).unwrap();
    s.conn()
        .execute(
            "INSERT INTO scan_jobs (id, root_path, tier, status, started_at, finished_at, errors_json)
             VALUES ('e9', ?1, 1, 'degraded', 9e15, 9e15, ?2)",
            rusqlite::params![root, errors(9)],
        )
        .unwrap();
    assert!(DriveErrorsWatch.run(&mut s, 1).unwrap().is_empty());
    s.conn()
        .execute(
            "INSERT INTO scan_jobs (id, root_path, tier, status, started_at, finished_at, errors_json)
             VALUES ('e14', ?1, 1, 'degraded', 9.1e15, 9.1e15, ?2)",
            rusqlite::params![root, errors(14)],
        )
        .unwrap();
    let f = DriveErrorsWatch.run(&mut s, 1).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].level, Level::Now);
    assert_eq!(f[0].subject, id);
    assert_eq!(f[0].title, "WD C had trouble reading 14 files");
    assert_eq!(f[0].detail, "Copy what matters off it soon.");
}

#[test]
fn self_check_reports_stale_heartbeat() {
    let mut s = store();
    record_run(s.conn(), "live_index", 0, None).unwrap();
    record_run(s.conn(), "fresh", 4_500, None).unwrap();
    let mut w = SelfCheckWatch::new(vec![("live_index", 1_000), ("fresh", 1_000)], None);
    let f = w.run(&mut s, 5_000).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].subject, "live_index");
    assert_eq!(f[0].level, Level::Note);
    assert_eq!(f[0].title, "Live file watching stopped working and was restarted");
}

#[test]
fn resume_only_previously_started_uncatalogued_drives() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let done = common::fake_drive(&tmp, "Done", "0001");
    let started = common::fake_drive(&tmp, "Started", "0002");
    let never = common::fake_drive(&tmp, "Never", "0003");
    common::plug_and_catalog(&mut s, &provider, &done);
    provider.plug(started.clone());
    provider.plug(never.clone());
    aegis_core::drives::sync_mounted(&mut s, &provider).unwrap();
    let started_id = common::volume_id_at(&s, &started.mount_point);
    s.conn()
        .execute(
            "INSERT INTO scan_jobs (id, root_path, tier, status, started_at) VALUES ('j', ?1, 1, 'interrupted', 1)",
            [common::canon(&started.mount_point)],
        )
        .unwrap();
    mark_catalog_requested(s.conn(), &started_id).unwrap();
    assert_eq!(catalogs_to_resume(s.conn()).unwrap(), vec![started_id]);
}

// ---------- Task 7: downloads janitor ----------

use aegis_core::engine::janitor::{JanitorKind, installer_matches, propose, scan_downloads};

fn aged(path: &Path, days: f64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let ago = std::time::Duration::from_secs_f64(days * 86_400.0);
    f.set_modified(std::time::SystemTime::now() - ago).unwrap();
}

/// A registered Downloads folder (so quarantine has a volume) and a store.
fn downloads() -> (tempfile::TempDir, SqliteStore, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let dl = tmp.path().join("Downloads");
    std::fs::create_dir_all(&dl).unwrap();
    let mut s = store();
    s.add_root(&dl).unwrap();
    scan_root(&mut s, &dl, &[1]).unwrap();
    let q = tmp.path().join(".quarantine");
    std::fs::create_dir_all(&q).unwrap();
    s.conn().execute("UPDATE volumes SET quarantine_dir = ?1", [q.to_string_lossy().to_string()]).unwrap();
    (tmp, s, dl)
}

fn now_ms() -> i64 {
    aegis_core::now_ms()
}

fn names(items: &[aegis_core::engine::janitor::JanitorItem]) -> Vec<String> {
    let mut v: Vec<String> = items
        .iter()
        .map(|i| Path::new(&i.path).file_name().unwrap().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

#[test]
fn partial_downloads_older_than_a_day() {
    let (_t, s, dl) = downloads();
    aged(&common::put(&dl, "movie.mkv.crdownload", b"partial"), 2.0);
    aged(&common::put(&dl, "fresh.part", b"partial"), 0.05);
    let items = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    assert_eq!(names(&items), vec!["movie.mkv.crdownload"]);
    assert_eq!(items[0].kind, JanitorKind::Partial);
}

#[test]
fn repeat_download_with_same_hash_flagged_keeps_oldest() {
    let (_t, s, dl) = downloads();
    aged(&common::put(&dl, "invoice.pdf", b"%PDF same bytes"), 5.0);
    aged(&common::put(&dl, "invoice (1).pdf", b"%PDF same bytes"), 1.0);
    aged(&common::put(&dl, "invoice (2).pdf", b"%PDF same bytes"), 0.5);
    let items = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    assert_eq!(names(&items), vec!["invoice (1).pdf", "invoice (2).pdf"]);
    assert!(items.iter().all(|i| i.kind == JanitorKind::Repeat { of: "invoice.pdf".into() }));
}

#[test]
fn repeat_name_but_different_content_not_flagged() {
    let (_t, s, dl) = downloads();
    aged(&common::put(&dl, "report.docx", b"PK version one"), 5.0);
    aged(&common::put(&dl, "report (1).docx", b"PK version two"), 1.0);
    assert!(scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap().is_empty());
}

#[test]
fn installer_age_and_match() {
    assert!(installer_matches("ZoomInstallerFull", &["Zoom Workplace".into()]));
    assert!(!installer_matches("Firefox Setup 133.0", &["Mozilla Firefox (x64 en-US)".into()]));
    assert!(installer_matches("vlc-3.0.21-win64", &["VLC media player".into()]));
    assert!(!installer_matches("vlc-3.0.21-win64", &["Zoom Workplace".into()]));

    let (_t, s, dl) = downloads();
    aged(&common::put(&dl, "ZoomInstallerFull.exe", b"MZ zoom"), 20.0);
    aged(&common::put(&dl, "tool-setup.msi", b"msi"), 30.0);
    aged(&common::put(&dl, "new-setup.exe", b"MZ new"), 3.0);
    let items = scan_downloads(s.conn(), &dl, &["Zoom Workplace".to_string()], now_ms()).unwrap();
    assert_eq!(names(&items), vec!["ZoomInstallerFull.exe", "tool-setup.msi"]);
    let zoom = items.iter().find(|i| i.path.ends_with("ZoomInstallerFull.exe")).unwrap();
    assert_eq!(zoom.kind, JanitorKind::Installer { already_installed: true, age_days: 20 });
    let tool = items.iter().find(|i| i.path.ends_with("tool-setup.msi")).unwrap();
    assert_eq!(tool.kind, JanitorKind::Installer { already_installed: false, age_days: 30 });
}

#[test]
fn janitor_skips_files_in_open_batch_and_missing_files() {
    let (_t, mut s, dl) = downloads();
    aged(&common::put(&dl, "a.crdownload", b"x"), 2.0);
    aged(&common::put(&dl, "b.crdownload", b"y"), 2.0);
    let first = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    assert_eq!(first.len(), 2);
    propose(&mut s, &first[..1]).unwrap().unwrap();
    std::fs::remove_file(dl.join("b.crdownload")).unwrap();
    assert!(scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap().is_empty());
}

#[test]
fn propose_once_while_batch_open() {
    let (_t, mut s, dl) = downloads();
    aged(&common::put(&dl, "a.crdownload", b"x"), 2.0);
    aged(&common::put(&dl, "b.crdownload", b"y"), 2.0);
    let items = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    let batch = propose(&mut s, &items[..1]).unwrap();
    assert!(batch.is_some());
    assert!(propose(&mut s, &items[1..]).unwrap().is_none());
    assert!(propose(&mut s, &[]).unwrap().is_none());
    let reason: String = s
        .conn()
        .query_row("SELECT reason FROM actions WHERE batch_id = ?1", [batch.unwrap()], |r| r.get(0))
        .unwrap();
    assert_eq!(reason, "Unfinished download, untouched for a day");
}

#[cfg(windows)]
#[test]
fn installed_programs_reads_the_registry() {
    let programs = aegis_core::engine::janitor::installed_programs();
    assert!(!programs.is_empty(), "every Windows install lists some programs");
    assert!(programs.iter().all(|p| !p.trim().is_empty() && !p.contains('\0')));
}

// ---------- Task 8: space samples and forecast ----------

use aegis_core::engine::space::{forecast, record_day};

const GIB: i64 = 1_073_741_824;

/// `days` daily samples ending 2026-11-(start+days-1); used grows by `per_day`, `free_at_end` left.
fn samples(s: &SqliteStore, days: usize, per_day: i64, free_at_end: i64) {
    let last_used = 100 * GIB + (days as i64 - 1) * per_day;
    let total = last_used + free_at_end;
    for i in 0..days {
        let day = format!("2026-11-{:02}", i + 1);
        record_day(s.conn(), &day, &[("C".into(), 100 * GIB + i as i64 * per_day, total)], &[]).unwrap();
    }
}

#[test]
fn forecast_needs_seven_samples() {
    let s = store();
    samples(&s, 6, 10 * GIB, 100 * GIB);
    assert!(forecast(s.conn(), "C", "2026-11-06").unwrap().is_none());
    record_day(s.conn(), "2026-11-07", &[("C".into(), 160 * GIB, 260 * GIB)], &[]).unwrap();
    assert!(forecast(s.conn(), "C", "2026-11-07").unwrap().is_some());
}

#[test]
fn steady_growth_gives_weeks_left() {
    let s = store();
    samples(&s, 10, 10 * GIB, 700 * GIB);
    let f = forecast(s.conn(), "C", "2026-11-10").unwrap().unwrap();
    assert!((f.weeks_left - 10.0).abs() < 0.1, "{}", f.weeks_left);
}

#[test]
fn shrinking_or_far_off_gives_none() {
    let s = store();
    samples(&s, 10, -GIB, 500 * GIB);
    assert!(forecast(s.conn(), "C", "2026-11-10").unwrap().is_none());
    let s = store();
    samples(&s, 10, GIB / 10, 500 * GIB); // ~0.1 GB/day: years away
    assert!(forecast(s.conn(), "C", "2026-11-10").unwrap().is_none());
}

#[test]
fn samples_older_than_four_weeks_are_ignored() {
    let s = store();
    samples(&s, 10, 10 * GIB, 700 * GIB);
    assert!(forecast(s.conn(), "C", "2026-12-31").unwrap().is_none());
}

#[test]
fn same_day_recorded_twice_keeps_one_row() {
    let s = store();
    record_day(s.conn(), "2026-11-01", &[("C".into(), 1, 10)], &[("R".into(), 5)]).unwrap();
    record_day(s.conn(), "2026-11-01", &[("C".into(), 2, 10)], &[("R".into(), 6)]).unwrap();
    let n: i64 = s.conn().query_row("SELECT COUNT(*) FROM space_samples", [], |r| r.get(0)).unwrap();
    let used: i64 = s.conn().query_row("SELECT used_bytes FROM space_samples", [], |r| r.get(0)).unwrap();
    let roots: i64 = s.conn().query_row("SELECT COUNT(*) FROM root_samples", [], |r| r.get(0)).unwrap();
    assert_eq!((n, used, roots), (1, 2, 1));
}

#[test]
fn fastest_root_picked() {
    let s = store();
    samples(&s, 10, 10 * GIB, 300 * GIB);
    for i in 0..10 {
        let day = format!("2026-11-{:02}", i + 1);
        record_day(
            s.conn(),
            &day,
            &[],
            &[("C:/Users/me/Downloads".into(), GIB * (1 + 7 * i)), ("C:/Users/me/Music".into(), GIB * (50 + i)), ("C:/Old".into(), GIB * (90 - i))],
        )
        .unwrap();
    }
    let f = forecast(s.conn(), "C", "2026-11-10").unwrap().unwrap();
    assert_eq!(f.fastest_root.as_deref(), Some("C:/Users/me/Downloads"));
    assert_eq!(f.fastest_root_bytes_per_week, 49 * GIB);
}

// ---------- Task 9: weekly note ----------

use aegis_core::engine::note::{LocalNow, build, due_week, ensure_first_day, latest, mark_read, save};
use aegis_core::engine::reminders::demoted_kinds;

fn local(date: &str, weekday: u8, hour: u8) -> LocalNow {
    LocalNow { date: date.into(), weekday, hour }
}

#[test]
fn due_on_note_day_after_hour_once_per_week() {
    let s = store();
    let c = s.conn();
    ensure_first_day(c, "2026-11-01").unwrap();
    // Sunday 2026-11-22 (weekday 6), note day Sunday 18:00.
    assert_eq!(due_week(c, &local("2026-11-22", 6, 17), 6, 18).unwrap(), Some("2026-11-15".into()));
    let mut note = build(c, "2026-11-15", 0, "2026-11-15", 1).unwrap();
    save(c, &note, 1).unwrap();
    assert_eq!(due_week(c, &local("2026-11-22", 6, 17), 6, 18).unwrap(), None, "before 18:00 the last note is current");
    assert_eq!(due_week(c, &local("2026-11-22", 6, 18), 6, 18).unwrap(), Some("2026-11-22".into()));
    note.week = "2026-11-22".into();
    save(c, &note, 2).unwrap();
    assert_eq!(due_week(c, &local("2026-11-22", 6, 20), 6, 18).unwrap(), None);
    assert_eq!(due_week(c, &local("2026-11-23", 0, 9), 6, 18).unwrap(), None);
}

#[test]
fn missed_note_day_catches_up_next_time() {
    let s = store();
    let c = s.conn();
    ensure_first_day(c, "2026-11-01").unwrap();
    // PC was off on Sunday 2026-11-22; on Monday the note for that Sunday is due.
    assert_eq!(due_week(c, &local("2026-11-23", 0, 9), 6, 18).unwrap(), Some("2026-11-22".into()));
}

#[test]
fn fresh_install_waits_for_its_first_note_day() {
    let s = store();
    let c = s.conn();
    ensure_first_day(c, "2026-11-23").unwrap();
    ensure_first_day(c, "2026-12-30").unwrap(); // first call wins
    assert_eq!(due_week(c, &local("2026-11-23", 0, 9), 6, 18).unwrap(), None);
    assert_eq!(due_week(c, &local("2026-11-29", 6, 18), 6, 18).unwrap(), Some("2026-11-29".into()));
}

#[test]
fn empty_sections_left_out() {
    let s = store();
    let n = build(s.conn(), "2026-11-22", 0, "2026-11-22", 1).unwrap();
    assert!(n.needs_you.is_empty() && n.did.is_empty() && n.space.is_empty() && n.about_aegis.is_empty());
    assert!(n.safety.is_none() && n.tidy.is_none());
    assert_eq!(n.headline, "A quiet week.");
}

#[test]
fn needs_you_capped_at_three_now_first() {
    let s = store();
    let c = s.conn();
    let conv: Vec<Finding> = (0..3).map(|i| finding("k", &format!("s{i}"), Level::Convenient, "c")).collect();
    sync_findings(c, &["k"], &conv, 1).unwrap();
    sync_findings(c, &["n"], &[finding("n", "x", Level::Now, "urgent")], 2).unwrap();
    let n = build(c, "2026-11-22", 0, "2026-11-22", 3).unwrap();
    assert_eq!(n.needs_you.len(), 3);
    assert_eq!(n.needs_you[0].level, Level::Now);
    assert_eq!(n.headline, "Needs you: 4 things.");
}

#[test]
fn headline_variants() {
    let s = store();
    let c = s.conn();
    sync_findings(c, &["k"], &[finding("k", "a", Level::Convenient, "c"), finding("k", "b", Level::Note, "d")], 1).unwrap();
    let n = build(c, "2026-11-22", 0, "2026-11-22", 2).unwrap();
    assert_eq!(n.headline, "A quiet week. 2 things to look at when you have a minute.");
}

#[test]
fn backed_up_headline_and_activity_lines() {
    let mut fx_store = store();
    let tmp = tempfile::TempDir::new().unwrap();
    let provider = FakeVolumeProvider::new();
    let drive = common::fake_drive(&tmp, "B", "0002");
    let id = common::plug_and_catalog(&mut fx_store, &provider, &drive);
    let c = fx_store.conn();
    c.execute("UPDATE volumes SET role = 'backup' WHERE id = ?1", [&id]).unwrap();
    c.execute(
        "INSERT INTO backup_jobs (id, backup_volume_id, authority, status, started_at, finished_at) VALUES ('b1', ?1, 'standing', 'completed', 5, 6)",
        [&id],
    )
    .unwrap();
    let n = build(c, "2026-11-22", 0, "2026-11-22", 10).unwrap();
    assert_eq!(n.safety.as_deref(), Some("Everything important is backed up."));
    assert_eq!(n.headline, "A quiet week. Everything important is backed up.");
    assert!(n.did.contains(&"Topped up the backup 1 time".to_string()), "{:?}", n.did);
    assert!(n.did.contains(&"Catalogued 1 drive".to_string()), "{:?}", n.did);
}

#[test]
fn demoted_kind_announced_once() {
    let s = store();
    let c = s.conn();
    let f = [finding("only_copy", "backup", Level::Convenient, "t")];
    sync_findings(c, &["only_copy"], &f, 0).unwrap();
    let id = list_open(c).unwrap()[0].id.clone();
    for t in 1..=3 {
        dismiss(c, &id, t).unwrap();
    }
    let first = build(c, "2026-11-22", 0, "2026-11-22", 5).unwrap();
    assert!(
        first.about_aegis.contains(&"I stopped reminding you about files that exist only on this PC; it's here instead.".to_string()),
        "{:?}",
        first.about_aegis
    );
    assert!(!first.about_aegis.iter().any(|l| l.contains("only_copy")), "no internal names in owner copy");
    assert_eq!(demoted_kinds(c).unwrap(), vec![("only_copy".to_string(), true)]);
    let second = build(c, "2026-11-29", 0, "2026-11-29", 6).unwrap();
    assert!(!second.about_aegis.iter().any(|l| l.contains("only on this PC")));
}

#[test]
fn saved_note_reads_back_and_marks_read() {
    let s = store();
    let c = s.conn();
    assert!(latest(c).unwrap().is_none());
    let n = build(c, "2026-11-22", 0, "2026-11-22", 1).unwrap();
    save(c, &n, 1).unwrap();
    assert!(!latest(c).unwrap().unwrap().read);
    mark_read(c, "2026-11-22", 2).unwrap();
    assert!(latest(c).unwrap().unwrap().read);
}

#[test]
fn file_matching_two_rules_is_proposed_once() {
    // A repeat download of an old installer ("ChromeSetup (2).exe") matches both rules.
    let (_t, mut s, dl) = downloads();
    aged(&common::put(&dl, "ChromeSetup (1).exe", b"MZ chrome setup"), 30.0);
    aged(&common::put(&dl, "ChromeSetup (2).exe", b"MZ chrome setup"), 29.0);
    let items = scan_downloads(s.conn(), &dl, &["Google Chrome".to_string()], now_ms()).unwrap();
    assert_eq!(names(&items), vec!["ChromeSetup (1).exe", "ChromeSetup (2).exe"], "one item per file");
    let repeat = items.iter().find(|i| i.path.ends_with("(2).exe")).unwrap();
    assert_eq!(repeat.kind, JanitorKind::Repeat { of: "ChromeSetup (1).exe".into() }, "repeat wins over installer");
    let batch = propose(&mut s, &items).unwrap().unwrap();
    let n: i64 = s.conn().query_row("SELECT COUNT(*) FROM actions WHERE batch_id = ?1", [batch], |r| r.get(0)).unwrap();
    assert_eq!(n, 2);
}

#[test]
fn file_with_an_action_elsewhere_is_skipped() {
    let (_t, mut s, dl) = downloads();
    aged(&common::put(&dl, "a.crdownload", b"x"), 2.0);
    aged(&common::put(&dl, "b.crdownload", b"y"), 2.0);
    let items = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    // Another feature already proposed one of these files, recorded with different path casing.
    let a = items.iter().find(|i| i.path.ends_with("a.crdownload")).unwrap().clone();
    let mut upper = a.clone();
    upper.path = a.path.to_uppercase();
    aegis_core::engine::janitor::propose(&mut s, &[upper]).unwrap().unwrap();
    s.conn().execute("UPDATE actions SET agent = 'dupes'", []).unwrap();
    let batch = propose(&mut s, &items).unwrap().unwrap();
    let paths: Vec<String> = {
        let mut st = s.conn().prepare("SELECT from_path FROM actions WHERE batch_id = ?1").unwrap();
        st.query_map([batch], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    assert_eq!(paths.len(), 1);
    assert!(paths[0].ends_with("b.crdownload"));
}

// ---------- Final review fixes ----------

use aegis_core::engine::watches::{TripwireLockWatch, mark_catalog_requested};

#[test]
fn windows_style_encrypt_and_rename_trips() {
    // On Windows a rename arrives as separate From/To events; ransomware usually writes
    // "name.ext.locked" and removes the original.
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..150 {
        let locked = root.join(format!("f{i}.txt.locked"));
        std::fs::write(&locked, common::noise(8192, i as u64)).unwrap();
        std::fs::remove_file(root.join(format!("f{i}.txt"))).unwrap();
        tw.observe(&file(&root, i), ChangeKind::Removed, 1_000);
        tw.observe(&PathBuf::from(common::canon(&locked)), ChangeKind::Created, 1_000);
    }
    let trips = tw.evaluate(s.conn(), 2_000).unwrap();
    assert_eq!(trips.len(), 1);
    assert!(trips[0].new_ext_renames >= 100, "{:?}", trips[0]);
    assert!(trips[0].suspect_encrypted >= 1, "random bytes under a new extension look scrambled");
}

#[test]
fn evaluate_rescans_root_at_most_once_per_second() {
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..250 {
        tw.observe(&file(&root, i), ChangeKind::Modified, 1_000);
    }
    tw.evaluate(s.conn(), 1_000).unwrap();
    let after_first = tw.scans();
    for t in 1..=10 {
        tw.observe(&file(&root, 250 + t), ChangeKind::Modified, 1_000 + t as i64 * 50);
        tw.evaluate(s.conn(), 1_000 + t as i64 * 50).unwrap();
    }
    assert_eq!(tw.scans(), after_first, "no rescans within a second");
    tw.evaluate(s.conn(), 2_100).unwrap();
    assert_eq!(tw.scans(), after_first + 1);
}

#[test]
fn tripwire_lock_keeps_reminder_open_until_released() {
    let mut s = store();
    aegis_core::backup::guard::lock_folder(&mut s, "C:/Users/me/Documents", "tripwire").unwrap();
    let f = TripwireLockWatch.run(&mut s, 1).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, "tripwire");
    assert_eq!(f[0].level, Level::Now);
    assert_eq!(f[0].subject, "C:/Users/me/Documents");
    aegis_core::backup::guard::release_lock(&mut s, None, "C:/Users/me/Documents").unwrap();
    assert!(TripwireLockWatch.run(&mut s, 2).unwrap().is_empty());
}

#[test]
fn batcher_requeue_returns_paths_on_next_drain() {
    let mut b = ChangeBatcher::new();
    b.push(PathBuf::from("C:/x/a.txt"), 0);
    let chunks = b.drain_ready(5_000);
    b.requeue(chunks.into_iter().flatten(), 5_000);
    assert!(b.drain_ready(5_100).is_empty(), "requeued paths wait for quiet again");
    assert_eq!(b.drain_ready(7_100).concat(), vec![PathBuf::from("C:/x/a.txt")]);
}

#[test]
fn batcher_drains_a_constantly_busy_file_after_max_wait() {
    let mut b = ChangeBatcher::new();
    for t in 0..40 {
        b.push(PathBuf::from("C:/x/outlook.pst"), t * 1_000);
        if t < 30 {
            assert!(b.drain_ready(t * 1_000 + 500).is_empty());
        }
    }
    assert_eq!(b.drain_ready(39_500).concat().len(), 1, "drained despite constant writes");
}

#[test]
fn index_changed_expands_moved_in_and_removed_folders() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let drive = common::fake_drive(&tmp, "A", "0001");
    let id = common::plug_and_catalog(&mut s, &provider, &drive);
    // A folder moved in: only the folder's own path is reported.
    for i in 0..3 {
        common::put(&drive.mount_point, &format!("Trip/p{i}.jpg"), b"jpg");
    }
    let folder = PathBuf::from(common::canon(&drive.mount_point.join("Trip")));
    index_changed(&mut s, &id, vec![vec![folder.clone()]]).unwrap();
    assert_eq!(common::live_file_paths(&s, &id).iter().filter(|p| p.contains("Trip")).count(), 3);
    // The folder deleted: again only its own path.
    std::fs::remove_dir_all(drive.mount_point.join("Trip")).unwrap();
    index_changed(&mut s, &id, vec![vec![folder]]).unwrap();
    assert_eq!(common::live_file_paths(&s, &id).iter().filter(|p| p.contains("Trip")).count(), 0);
}

#[test]
fn only_copy_silent_while_backup_drive_is_plugged_in() {
    assert!(aegis_core::engine::watches::only_copy_finding_online(GB * 5, 30, true, "B").is_none());
    assert!(aegis_core::engine::watches::only_copy_finding_online(GB * 5, 30, false, "B").is_some());
}

#[test]
fn drive_errors_ignore_permission_denied() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let drive = common::fake_drive(&tmp, "Old system disk", "0004");
    common::plug_and_catalog(&mut s, &provider, &drive);
    let root = common::canon(&drive.mount_point);
    let denied = serde_json::to_string(&vec![serde_json::json!({"path": "x", "code": "permission_denied", "message": "m"}); 30]).unwrap();
    s.conn()
        .execute(
            "INSERT INTO scan_jobs (id, root_path, tier, status, started_at, finished_at, errors_json)
             VALUES ('p', ?1, 1, 'degraded', 9e15, 9e15, ?2)",
            rusqlite::params![root, denied],
        )
        .unwrap();
    assert!(DriveErrorsWatch.run(&mut s, 1).unwrap().is_empty());
}

#[test]
fn resume_requires_an_owner_request_for_that_drive() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let asked = common::fake_drive(&tmp, "Asked", "0002");
    let stranger = common::fake_drive(&tmp, "Stranger", "0003");
    provider.plug(asked.clone());
    provider.plug(stranger.clone());
    aegis_core::drives::sync_mounted(&mut s, &provider).unwrap();
    let asked_id = common::volume_id_at(&s, &asked.mount_point);
    // Both drives' letters have old jobs (a reused letter), but only one was ever asked for.
    for (j, d) in [("j1", &asked), ("j2", &stranger)] {
        s.conn()
            .execute(
                "INSERT INTO scan_jobs (id, root_path, tier, status, started_at) VALUES (?1, ?2, 1, 'interrupted', 1)",
                rusqlite::params![j, common::canon(&d.mount_point)],
            )
            .unwrap();
    }
    mark_catalog_requested(s.conn(), &asked_id).unwrap();
    assert_eq!(catalogs_to_resume(s.conn()).unwrap(), vec![asked_id]);
}

#[test]
fn janitor_skips_files_the_owner_rejected() {
    let (_t, mut s, dl) = downloads();
    aged(&common::put(&dl, "a.crdownload", b"x"), 2.0);
    let items = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    let batch = propose(&mut s, &items).unwrap().unwrap();
    aegis_core::db::actions::reject_batch(&mut s, &batch, Some("keep these")).unwrap();
    assert!(scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap().is_empty());
}

#[test]
fn executables_inside_subfolders_are_not_installers() {
    let (_t, s, dl) = downloads();
    aged(&common::put(&dl, "tool/bin/tool.exe", b"MZ portable"), 60.0);
    aged(&common::put(&dl, "setup.exe", b"MZ setup"), 60.0);
    let items = scan_downloads(s.conn(), &dl, &[], now_ms()).unwrap();
    assert_eq!(names(&items), vec!["setup.exe"]);
}

#[test]
fn self_check_copy_is_plain_language() {
    let mut s = store();
    record_run(s.conn(), "live_index", 0, None).unwrap();
    let mut w = SelfCheckWatch::new(vec![("live_index", 1_000)], None);
    let f = w.run(&mut s, 5_000).unwrap();
    assert_eq!(f[0].title, "Live file watching stopped working and was restarted");
}

#[test]
fn copying_many_files_of_a_new_type_does_not_trip() {
    // 300 compressed photos of a type the folder never had (random-looking bytes), nothing removed.
    let (_t, s, root, canon) = indexed_root(20);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..300 {
        let p = common::put(&root, &format!("raw/IMG_{i}.cr3"), &common::noise(8192, i as u64));
        tw.observe(&PathBuf::from(common::canon(&p)), ChangeKind::Created, 1_000);
    }
    assert!(tw.evaluate(s.conn(), 2_000).unwrap().is_empty(), "a big copy-in is not an attack");
}

#[test]
fn random_named_encrypted_replacements_trip() {
    // Some ransomware writes random names; the originals still disappear.
    let (_t, s, root, canon) = indexed_root(300);
    let mut tw = Tripwire::new(vec![canon]);
    for i in 0..150 {
        let out = common::put(&root, &format!("{i:08x}.crypt"), &common::noise(8192, 900 + i as u64));
        std::fs::remove_file(root.join(format!("f{i}.txt"))).unwrap();
        tw.observe(&file(&root, i), ChangeKind::Removed, 1_000);
        tw.observe(&PathBuf::from(common::canon(&out)), ChangeKind::Created, 1_000);
    }
    let trips = tw.evaluate(s.conn(), 2_000).unwrap();
    assert_eq!(trips.len(), 1);
    assert!(trips[0].suspect_encrypted >= 1);
}

#[test]
fn is_live_excluded_path_covers_build_cache_and_git_only() {
    use aegis_core::scanner::is_live_excluded_path;
    let tmp = tempfile::TempDir::new().unwrap();
    let app = tmp.path().join("app");
    let cargo_app = tmp.path().join("rust-app");
    std::fs::create_dir_all(app.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(app.join(".venv/Lib")).unwrap();
    std::fs::create_dir_all(app.join(".git/objects")).unwrap();
    std::fs::create_dir_all(app.join(".aegis-quarantine/b1")).unwrap();
    std::fs::create_dir_all(app.join("target")).unwrap();
    std::fs::create_dir_all(cargo_app.join("target/debug")).unwrap();
    std::fs::write(cargo_app.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

    assert!(is_live_excluded_path(&app.join("node_modules/pkg/index.js")));
    assert!(is_live_excluded_path(&app.join(".venv/Lib/site.py")));
    assert!(is_live_excluded_path(&app.join(".git/index.lock")));
    assert!(is_live_excluded_path(&app.join(".aegis-quarantine/b1/old.txt")));
    assert!(is_live_excluded_path(&cargo_app.join("target/debug/app.exe")));

    assert!(!is_live_excluded_path(&app.join("src/main.rs")));
    assert!(!is_live_excluded_path(&app.join("target/notes.md")), "plain target folder without Cargo.toml or CACHEDIR.TAG is user data");
}

#[test]
fn index_changed_skips_excluded_subdirs_and_handles_vanished_files_and_folders() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut s = store();
    let provider = FakeVolumeProvider::new();
    let drive = common::fake_drive(&tmp, "A", "0001");
    let id = common::plug_and_catalog(&mut s, &provider, &drive);

    let proj = drive.mount_point.join("MyApp");
    common::put(&proj, "Cargo.toml", b"[package]\nname = \"myapp\"\n");
    common::put(&proj, "src/main.rs", b"fn main() {}");
    common::put(&proj, "docs/guide.md", b"# guide");
    common::put(&proj, "node_modules/left-pad/index.js", b"module.exports = 1;");
    common::put(&proj, ".venv/Lib/site.py", b"# venv");
    common::put(&proj, ".git/objects/00/abcdef", b"obj");
    common::put(&proj, "target/debug/myapp.exe", b"exe");

    let proj_canon = PathBuf::from(common::canon(&proj));
    index_changed(&mut s, &id, vec![vec![proj_canon.clone()]]).unwrap();
    let paths = common::live_file_paths(&s, &id);
    assert!(paths.iter().any(|p| p.ends_with("main.rs")), "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("guide.md")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("node_modules") || p.contains(".venv") || p.contains(".git") || p.contains("target")), "{paths:?}");

    // Vanished single file removes only that file via O(log N) lookup.
    let vanished_file = PathBuf::from(common::canon(&proj.join("docs/guide.md")));
    std::fs::remove_file(proj.join("docs/guide.md")).unwrap();
    index_changed(&mut s, &id, vec![vec![vanished_file]]).unwrap();
    let paths_after_file = common::live_file_paths(&s, &id);
    assert!(!paths_after_file.iter().any(|p| p.ends_with("guide.md")), "{paths_after_file:?}");
    assert!(paths_after_file.iter().any(|p| p.ends_with("main.rs")), "{paths_after_file:?}");

    // Vanished folder removes all remaining indexed children beneath it.
    std::fs::remove_dir_all(&proj).unwrap();
    index_changed(&mut s, &id, vec![vec![proj_canon]]).unwrap();
    assert!(!common::live_file_paths(&s, &id).iter().any(|p| p.contains("MyApp")));
}
