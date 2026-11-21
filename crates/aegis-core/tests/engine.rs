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
