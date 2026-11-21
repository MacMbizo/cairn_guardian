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
