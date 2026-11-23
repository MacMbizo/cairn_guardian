//! Runs the always-on engine (spec §6, §7) on two threads, each with its own database
//! connection:
//!
//! - the **event thread** keeps the live index current and runs the ransomware tripwire. It
//!   never does heavy work, so an attack is noticed even while a tidy-up is hashing files;
//! - the **scheduler thread** (below-normal priority) runs the watches, the weekly note,
//!   reminder delivery and the tray icon state.

use crate::commands::{AEGIS_EVENT, AppState};
use crate::shell::{open_main, set_tray_state};
use aegis_core::contracts::AegisEvent;
use aegis_core::db::SqliteStore;
use aegis_core::db::files::{normalize_path, path_is_within};
use aegis_core::engine::batcher::{ChangeBatcher, index_changed};
use aegis_core::engine::janitor::{JanitorWatch, installed_programs};
use aegis_core::engine::note::{LocalNow, build, due_week, ensure_first_day, save};
use aegis_core::engine::reminders::{Reminder, act, dismiss, list_open, mark_shown, sync_findings, to_deliver, tray_state};
use aegis_core::engine::space::SpaceWatch;
use aegis_core::engine::tripwire::{ChangeKind, Trip, Tripwire, trip_finding};
use aegis_core::engine::watches::{
    DriveErrorsWatch, OnlyCopyWatch, QuarantineExpiryWatch, SelfCheckWatch, TripwireLockWatch, catalogs_to_resume,
};
use aegis_core::engine::{Calm, Engine, FindingAction, Screen, record_run};
use aegis_core::now_ms;
use aegis_core::scanner::is_live_excluded_path;
use chrono::{Datelike, Local, Timelike};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Messages to the event thread.
pub enum EngineMsg {
    Fs(notify::Event),
    /// The file watcher reported an error (lost events, a removed root): rebuild it.
    WatchError(String),
    /// Something changed (a command, pause): re-check reminders, tray and the watched roots.
    Wake,
}

/// How often the scheduler runs.
const SLOW_TICK: Duration = Duration::from_secs(30);
/// While changes are pending, the event thread wakes this often to apply them.
const FAST_TICK: Duration = Duration::from_millis(500);
/// How often the event thread re-checks its roots and records its heartbeat.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Cadence of the live index heartbeat the self-check watches.
const LIVE_INDEX_EVERY_MS: i64 = 60_000;
/// Attempts to write a tripwire lock before giving up for this burst.
const LOCK_ATTEMPTS: usize = 5;

pub fn wake(app: &AppHandle) {
    let _ = app.state::<AppState>().engine_tx.lock().map(|tx| tx.send(EngineMsg::Wake));
}

/// What the PC is doing now: idle time, power and whether a game/presentation is running.
#[cfg(windows)]
pub fn calm_now() -> Calm {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows_sys::Win32::UI::Shell::{
        QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
    };
    // SAFETY: plain Win32 queries writing into zeroed out-structs owned by this frame.
    unsafe {
        let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        let idle_secs = if GetLastInputInfo(&mut lii) != 0 {
            u64::from(GetTickCount().wrapping_sub(lii.dwTime)) / 1000
        } else {
            0
        };
        let mut ps: SYSTEM_POWER_STATUS = std::mem::zeroed();
        let (on_ac, battery_pct) = if GetSystemPowerStatus(&mut ps) != 0 {
            let no_battery = ps.BatteryFlag == 128;
            (ps.ACLineStatus == 1 || no_battery, (ps.BatteryLifePercent <= 100).then_some(ps.BatteryLifePercent))
        } else {
            (true, None)
        };
        let mut quns = 0;
        let busy = SHQueryUserNotificationState(&mut quns) == 0
            && [QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, QUNS_PRESENTATION_MODE].contains(&quns);
        Calm { idle_secs, on_ac, battery_pct, busy }
    }
}

#[cfg(not(windows))]
pub fn calm_now() -> Calm {
    Calm { idle_secs: 0, on_ac: true, battery_pct: None, busy: false }
}

/// Heavy work yields to whatever the owner is doing.
fn lower_priority() {
    #[cfg(windows)]
    // SAFETY: changes only the calling thread's scheduling priority.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

fn screen_of(action: &FindingAction) -> Screen {
    match action {
        FindingAction::Open { screen } => *screen,
        FindingAction::Batch { .. } => Screen::Simulator,
        FindingAction::None => Screen::Week,
    }
}

/// Only the owner closing a notification counts as "not now"; it timing out does not.
#[cfg(windows)]
pub fn dismissal_counts(reason: Option<tauri_winrt_notification::ToastDismissalReason>) -> bool {
    matches!(reason, Some(tauri_winrt_notification::ToastDismissalReason::UserCanceled))
}

/// A quiet Windows notification. Clicking it opens `screen`; for a reminder, clicking counts as
/// acting on it and closing it counts as "not now" (feeding the learn-from-dismissals rule).
#[cfg(windows)]
pub fn toast(app: &AppHandle, title: &str, detail: &str, screen: Screen, reminder_id: Option<String>) {
    use tauri_winrt_notification::Toast;
    // Installed builds own an app identity; a raw dev exe borrows PowerShell's so toasts still show.
    let installed = std::env::current_exe()
        .map(|p| !p.to_string_lossy().to_lowercase().contains(r"\target\"))
        .unwrap_or(false);
    let id = if installed { app.config().identifier.clone() } else { Toast::POWERSHELL_APP_ID.to_string() };
    let (on_click, on_close) = (app.clone(), app.clone());
    let (click_id, close_id) = (reminder_id.clone(), reminder_id);
    let _ = Toast::new(&id)
        .title(title)
        .text1(detail)
        .sound(None)
        .on_activated(move |_| {
            if let Some(rid) = &click_id {
                with_ui_store(&on_click, |c| act(c, rid, now_ms()).map(|_| ()));
            }
            open_main(&on_click, Some(screen));
            Ok(())
        })
        .on_dismissed(move |reason| {
            if dismissal_counts(reason)
                && let Some(rid) = &close_id
            {
                with_ui_store(&on_close, |c| dismiss(c, rid, now_ms()).map(|_| ()));
            }
            Ok(())
        })
        .show();
}

#[cfg(not(windows))]
pub fn toast(_app: &AppHandle, _title: &str, _detail: &str, _screen: Screen, _reminder_id: Option<String>) {}

/// Runs `f` on the UI's connection, then wakes the engine so the tray catches up.
fn with_ui_store(app: &AppHandle, f: impl FnOnce(&rusqlite::Connection) -> aegis_core::error::Result<()>) {
    if let Ok(store) = app.state::<AppState>().store.lock() {
        let _ = f(store.conn());
    }
    wake(app);
}

fn deliver(app: &AppHandle, r: &Reminder) {
    toast(app, &r.title, &r.detail, screen_of(&r.action), Some(r.id.clone()));
}

/// "Back up now" from the tray.
pub fn backup_now(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(mut store) = app.state::<AppState>().open_store() else { return };
        if let Ok(Some((id, _, true))) = aegis_core::backup::backup_volume(store.conn()) {
            crate::watch::run_topup_job(&app, &mut store, &id, aegis_core::backup::topup::TopUpAuthority::UserApproved);
        }
    });
}

fn setting_u8(store: &SqliteStore, key: &str, default: u8) -> u8 {
    store.get_setting(key, "global").ok().flatten().and_then(|v| v.trim_matches('"').parse().ok()).unwrap_or(default)
}

fn local_now() -> LocalNow {
    let t = Local::now();
    LocalNow {
        date: t.format("%Y-%m-%d").to_string(),
        weekday: t.weekday().num_days_from_monday() as u8,
        hour: t.hour() as u8,
    }
}

fn enabled_roots(store: &SqliteStore) -> Vec<(String, String)> {
    store
        .list_roots()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.enabled)
        .map(|r| (r.path, r.volume_id))
        .collect()
}

fn change_kind(ev: &notify::Event) -> Vec<(PathBuf, ChangeKind)> {
    use notify::EventKind;
    use notify::event::{ModifyKind, RenameMode};
    match &ev.kind {
        EventKind::Create(_) => ev.paths.iter().map(|p| (p.clone(), ChangeKind::Created)).collect(),
        EventKind::Remove(_) => ev.paths.iter().map(|p| (p.clone(), ChangeKind::Removed)).collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if ev.paths.len() == 2 => {
            vec![(ev.paths[0].clone(), ChangeKind::Renamed { to: ev.paths[1].clone() })]
        }
        // Windows reports renames as separate From/To events: the tripwire sees them as a
        // removal plus a creation, which it counts toward "new extension" files.
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            ev.paths.iter().map(|p| (p.clone(), ChangeKind::Removed)).collect()
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            ev.paths.iter().map(|p| (p.clone(), ChangeKind::Created)).collect()
        }
        EventKind::Modify(_) => ev.paths.iter().map(|p| (p.clone(), ChangeKind::Modified)).collect(),
        _ => Vec::new(),
    }
}

fn display(p: &std::path::Path) -> PathBuf {
    PathBuf::from(normalize_path(p, false).0)
}

/// Locks the folder's backup, retrying when the database is busy. Returns whether it stuck.
fn lock_with_retry(store: &mut SqliteStore, trip: &Trip) -> bool {
    for attempt in 0..LOCK_ATTEMPTS {
        if aegis_core::backup::guard::lock_folder(store, &trip.root, "tripwire").is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200 * (attempt as u64 + 1)));
    }
    false
}

/// Starts the engine. `rx` receives file-system events and wake-ups.
pub fn spawn(app: AppHandle, rx: Receiver<EngineMsg>) {
    let (sched_tx, sched_rx) = std::sync::mpsc::channel::<()>();
    spawn_scheduler(app.clone(), sched_rx);
    std::thread::spawn(move || event_loop(app, rx, sched_tx));
}

fn push_fs_event(ev: notify::Event, batcher: &mut ChangeBatcher, tripwire: &mut Tripwire, now: i64) {
    for (path, kind) in change_kind(&ev) {
        let shown = display(&path);
        match kind {
            ChangeKind::Renamed { to } => {
                let to_shown = display(&to);
                let from_excl = is_live_excluded_path(&shown);
                let to_excl = is_live_excluded_path(&to_shown);
                if from_excl && to_excl {
                    continue;
                }
                if !from_excl {
                    batcher.push(shown.clone(), now);
                }
                if !to_excl {
                    batcher.push(to_shown.clone(), now);
                }
                tripwire.observe(&shown, ChangeKind::Renamed { to: to_shown }, now);
            }
            other => {
                if is_live_excluded_path(&shown) {
                    continue;
                }
                // Windows fires Modify on parent directories whenever any child file changes;
                // expanding a Modified dir would re-walk the entire subtree on every write.
                if !(matches!(other, ChangeKind::Modified) && shown.is_dir()) {
                    batcher.push(shown.clone(), now);
                }
                tripwire.observe(&shown, other, now);
            }
        }
    }
}

/// Live index and tripwire. Light work only.
fn event_loop(app: AppHandle, rx: Receiver<EngineMsg>, sched: Sender<()>) {
    let state = app.state::<AppState>();
    let Ok(mut store) = state.open_store() else { return };
    let mut roots = enabled_roots(&store);
    let mut watcher = start_watcher(&app, &roots);
    let mut watch_error: Option<String> = None;
    let mut tripwire = Tripwire::new(roots.iter().map(|r| r.0.clone()).collect());
    let mut batcher = ChangeBatcher::new();
    let mut last_beat = std::time::Instant::now() - HEARTBEAT;

    loop {
        let timeout = if batcher.is_empty() { HEARTBEAT } else { FAST_TICK };
        let mut woke = false;
        let mut disconnected = false;
        match rx.recv_timeout(timeout) {
            Ok(EngineMsg::Fs(ev)) => {
                let now = now_ms();
                push_fs_event(ev, &mut batcher, &mut tripwire, now);
                for _ in 0..2_000 {
                    match rx.try_recv() {
                        Ok(EngineMsg::Fs(more)) => push_fs_event(more, &mut batcher, &mut tripwire, now),
                        Ok(EngineMsg::WatchError(e)) => {
                            watch_error = Some(e);
                            watcher = None;
                            break;
                        }
                        Ok(EngineMsg::Wake) => {
                            woke = true;
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }
            Ok(EngineMsg::WatchError(e)) => {
                // Events may have been lost: rebuild the watcher at once.
                watch_error = Some(e);
                watcher = None;
            }
            Ok(EngineMsg::Wake) => woke = true,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if disconnected {
            break;
        }
        let now = now_ms();

        // Live index: apply quiet bursts in small transactions, grouped by root's volume.
        let mut applied = 0usize;
        for chunk in batcher.drain_ready(now) {
            for (root, vol) in &roots {
                let mine: Vec<PathBuf> =
                    chunk.iter().filter(|p| path_is_within(&p.to_string_lossy(), root, false)).cloned().collect();
                if !mine.is_empty() {
                    match index_changed(&mut store, vol, vec![mine.clone()]) {
                        Ok(n) => applied += n,
                        Err(_) => batcher.requeue(mine, now), // database busy: try again shortly
                    }
                }
            }
        }
        if applied >= 100 {
            store.shrink_memory();
        }

        // Tripwire: lock the folder's backup and tell the owner at once.
        if let Ok(trips) = tripwire.evaluate(store.conn(), now) {
            for trip in trips {
                if !lock_with_retry(&mut store, &trip) {
                    let _ = record_run(store.conn(), "tripwire", now, Some("could not lock the folder's backup"));
                }
                let _ = sync_findings(store.conn(), &[], std::slice::from_ref(&trip_finding(&trip)), now);
                let _ = app.emit(AEGIS_EVENT, AegisEvent::Tripped { v: 1, trip: trip.clone() });
                woke = true;
            }
        }

        if woke {
            let _ = sched.send(());
        }
        if !(woke || watcher.is_none() || last_beat.elapsed() >= HEARTBEAT) {
            continue;
        }
        last_beat = std::time::Instant::now();

        // Roots may have changed (owner added a folder), or the watcher failed: (re)build it.
        let now_roots = enabled_roots(&store);
        if now_roots != roots || watcher.is_none() {
            roots = now_roots;
            watcher = start_watcher(&app, &roots);
            tripwire = Tripwire::new(roots.iter().map(|r| r.0.clone()).collect());
        }
        // Heartbeat: healthy only when the watcher is running and reported no error.
        let err = if watcher.is_none() { Some("the file watcher could not start".to_string()) } else { watch_error.take() };
        let _ = record_run(store.conn(), "live_index", now, err.as_deref());
    }
}

/// Watches, weekly note, reminders and tray. Runs below normal priority on its own connection.
fn spawn_scheduler(app: AppHandle, wake_rx: Receiver<()>) {
    std::thread::spawn(move || {
        lower_priority();
        let state = app.state::<AppState>();
        let db_path = state.db_path.clone();
        let mut store = match state.open_store() {
            Ok(s) => s,
            Err(e) => {
                let _ = app.emit(AEGIS_EVENT, AegisEvent::JobFailed { v: 1, job: "engine".into(), message: e.to_string() });
                return;
            }
        };
        let _ = aegis_core::scanner::mark_interrupted_jobs(store.conn(), now_ms());
        let _ = ensure_first_day(store.conn(), &local_now().date);
        if let Ok(p) = store.get_setting("engine.paused_until", "global")
            && let Some(v) = p.and_then(|v| v.trim_matches('"').parse::<i64>().ok())
        {
            state.paused_until.store(v, Ordering::SeqCst);
        }
        for id in catalogs_to_resume(store.conn()).unwrap_or_default() {
            crate::commands::drives::start_catalog(&app, id);
        }

        let vols: Arc<Mutex<Vec<(String, i64, i64)>>> = Arc::default();
        let vols_for_watch = vols.clone();
        let mut watches: Vec<Box<dyn aegis_core::engine::Watch>> = vec![
            Box::new(TripwireLockWatch),
            Box::new(OnlyCopyWatch),
            Box::new(QuarantineExpiryWatch),
            Box::new(DriveErrorsWatch),
            Box::new(SelfCheckWatch::new(vec![("live_index", LIVE_INDEX_EVERY_MS)], Some(db_path.clone()))),
            Box::new(SpaceWatch {
                volumes: Box::new(move || vols_for_watch.lock().map(|v| v.clone()).unwrap_or_default()),
                today: Box::new(|| Local::now().format("%Y-%m-%d").to_string()),
            }),
        ];
        watches.push(Box::new(aegis_core::filing::reader::ReaderWatch));
        // Recognising projects comes before filing, so filing never splits one.
        watches.push(Box::new(aegis_core::context::watch::ContextWatch));
        watches.push(Box::new(aegis_core::filing::watch::FilingWatch::default()));
        if let Some(dl) = aegis_core::scanner::downloads_dir() {
            watches.push(Box::new(JanitorWatch { downloads: dl, installed: Box::new(installed_programs) }));
        }
        let mut engine = Engine::new(watches);

        loop {
            let woke = match wake_rx.recv_timeout(SLOW_TICK) {
                Ok(()) => {
                    while wake_rx.try_recv().is_ok() {}
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let now = now_ms();

            if let Ok(list) = store.list_volumes() {
                let online: Vec<(String, i64, i64)> = list
                    .into_iter()
                    .filter(|v| v.status == "online" && v.total_bytes > 0)
                    .map(|v| (v.id, (v.total_bytes - v.free_bytes) as i64, v.total_bytes as i64))
                    .collect();
                if let Ok(mut g) = vols.lock() {
                    *g = online;
                }
            }

            let calm = calm_now();
            let paused = state.paused_until.load(Ordering::SeqCst) > now;
            let mut changed = woke;
            if let Ok(report) = engine.tick(&mut store, now, &calm, paused) {
                changed |= report.ran.iter().any(|n| *n != "tripwire_locks");
                if report.ran.iter().any(|n| matches!(*n, "reader" | "context" | "filing" | "janitor")) {
                    store.shrink_memory();
                }
            }

            // The weekly note.
            let local = local_now();
            let day = setting_u8(&store, "engine.note_day", 6);
            let hour = setting_u8(&store, "engine.note_hour", 18);
            if let Ok(Some(week)) = due_week(store.conn(), &local, day, hour)
                && let Ok(note) = build(store.conn(), &week, now - 7 * 86_400_000, &local.date, now)
                && save(store.conn(), &note, now).is_ok()
            {
                toast(&app, "Your weekly note is ready", &note.headline, Screen::Week, None);
                changed = true;
            }

            // Reminders that are due now.
            if let Ok(due) = to_deliver(store.conn(), now, calm.busy, paused) {
                changed |= !due.is_empty();
                for r in &due {
                    deliver(&app, r);
                    let _ = mark_shown(store.conn(), &r.id, now);
                }
            }

            // Tray icon and the "Back up now" item.
            let open = list_open(store.conn()).map(|v| v.len()).unwrap_or(0);
            if let Ok(t) = tray_state(store.conn()) {
                set_tray_state(&app, t, open, paused);
            }
            let backup_online = matches!(aegis_core::backup::backup_volume(store.conn()), Ok(Some((_, _, true))));
            if let Some(item) = state.tray_items.lock().ok().and_then(|g| g.as_ref().map(|i| i.backup_now.clone())) {
                let _ = item.set_enabled(backup_online);
            }
            if changed {
                let _ = app.emit(AEGIS_EVENT, AegisEvent::EngineUpdated { v: 1 });
            }
        }
    });
}

/// Watches the PC roots recursively; events (and watcher errors) go to the event thread.
fn start_watcher(app: &AppHandle, roots: &[(String, String)]) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let tx = app.state::<AppState>().engine_tx.lock().ok()?.clone();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let _ = match res {
            Ok(ev) => tx.send(EngineMsg::Fs(ev)),
            Err(e) => tx.send(EngineMsg::WatchError(e.to_string())),
        };
    })
    .ok()?;
    let mut any = false;
    for (root, _) in roots {
        if w.watch(std::path::Path::new(root), RecursiveMode::Recursive).is_ok() {
            any = true;
        }
    }
    any.then_some(w)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn calm_probe_does_not_panic() {
        let c = super::calm_now();
        assert!(c.battery_pct.is_none_or(|p| p <= 100));
    }

    #[cfg(windows)]
    #[test]
    fn only_a_user_close_counts_as_not_now() {
        use tauri_winrt_notification::ToastDismissalReason as R;
        assert!(super::dismissal_counts(Some(R::UserCanceled)));
        assert!(!super::dismissal_counts(Some(R::TimedOut)));
        assert!(!super::dismissal_counts(Some(R::ApplicationHidden)));
        assert!(!super::dismissal_counts(None));
    }

    #[test]
    fn push_fs_event_skips_excluded_paths_and_dir_modify() {
        use aegis_core::engine::batcher::ChangeBatcher;
        use aegis_core::engine::tripwire::Tripwire;
        use notify::{Event, EventKind, event::{DataChange, ModifyKind}};

        let base = std::env::temp_dir().join(format!("aegis-eh-test-{}", aegis_core::new_id()));
        let root = base.join("Prj");
        let nm_file = root.join("app/node_modules/pkg/a.js");
        let dir_only = root.join("app");
        let code_file = root.join("app/src/main.rs");
        std::fs::create_dir_all(&dir_only).unwrap();
        std::fs::create_dir_all(code_file.parent().unwrap()).unwrap();
        std::fs::write(&code_file, "fn main() {}").unwrap();

        let mut batcher = ChangeBatcher::new();
        let mut tw = Tripwire::new(vec![root.to_string_lossy().to_string()]);

        let ev = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(nm_file)
            .add_path(dir_only.clone())
            .add_path(code_file);
        super::push_fs_event(ev, &mut batcher, &mut tw, 0);

        let drained = batcher.drain_ready(10_000).concat();
        assert_eq!(drained.len(), 1, "only the real file modify should be queued: {drained:?}");
        assert!(drained[0].ends_with("main.rs"));

        // Creating a folder still expands so newly moved-in folders are indexed.
        let mut batcher2 = ChangeBatcher::new();
        let created_dir = Event::new(EventKind::Create(notify::event::CreateKind::Folder)).add_path(dir_only);
        super::push_fs_event(created_dir, &mut batcher2, &mut tw, 0);
        assert_eq!(batcher2.drain_ready(10_000).concat().len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
