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
