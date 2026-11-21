//! Always-on engine: scheduling, reminders, watches, tripwire, janitor, space and the weekly note.

#[allow(dead_code)]
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
    assert!(is_due(1000, None, 0, true, false));
    assert!(is_due(1000, Some(0), 1000, true, true));
    assert!(!is_due(1000, Some(0), 999, false, true));
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
    let errors = |n: usize| serde_json::to_string(&vec![serde_json::json!({"path": "x", "reason": "io"}); n]).unwrap();
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
    assert_eq!(f[0].title, "live_index stopped working and was restarted");
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
