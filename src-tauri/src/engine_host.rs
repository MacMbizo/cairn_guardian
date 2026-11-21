//! Runs the always-on engine (spec §6, §7) on its own thread with its own database connection:
//! live folder watching, the ransomware tripwire, scheduled watches, gentle reminders, the
//! weekly note and the tray icon state.

use crate::commands::{AEGIS_EVENT, AppState};
use crate::shell::{open_main, set_tray_state};
use aegis_core::contracts::AegisEvent;
use aegis_core::db::SqliteStore;
use aegis_core::db::files::{normalize_path, path_is_within};
use aegis_core::engine::batcher::{ChangeBatcher, index_changed};
use aegis_core::engine::janitor::{JanitorWatch, installed_programs};
use aegis_core::engine::note::{LocalNow, build, due_week, ensure_first_day, save};
use aegis_core::engine::reminders::{Reminder, list_open, mark_shown, sync_findings, to_deliver, tray_state};
use aegis_core::engine::space::SpaceWatch;
use aegis_core::engine::tripwire::{ChangeKind, Tripwire, trip_finding};
use aegis_core::engine::watches::{
    DriveErrorsWatch, OnlyCopyWatch, QuarantineExpiryWatch, SelfCheckWatch, catalogs_to_resume,
};
use aegis_core::engine::{Calm, Engine, FindingAction, Screen, record_run};
use aegis_core::now_ms;
use chrono::{Datelike, Local, Timelike};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Messages to the engine thread.
pub enum EngineMsg {
    Fs(notify::Event),
    /// Something changed (a command, pause): re-check reminders, tray and the watcher's roots.
    Wake,
}

/// How often the scheduled part of the engine runs.
const SLOW_TICK: Duration = Duration::from_secs(30);
/// While changes are pending, the loop wakes this often to apply them once things are quiet.
const FAST_TICK: Duration = Duration::from_millis(500);
/// Cadence of the live index heartbeat the self-check watches.
const LIVE_INDEX_EVERY_MS: i64 = 60_000;

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

fn screen_of(action: &FindingAction) -> Screen {
    match action {
        FindingAction::Open { screen } => *screen,
        FindingAction::Batch { .. } => Screen::Simulator,
        FindingAction::None => Screen::Week,
    }
}

/// A quiet Windows notification. Clicking it opens `screen`.
#[cfg(windows)]
pub fn toast(app: &AppHandle, title: &str, detail: &str, screen: Screen) {
    use tauri_winrt_notification::Toast;
    // Installed builds own an app identity; a raw dev exe borrows PowerShell's so toasts still show.
    let installed = std::env::current_exe()
        .map(|p| !p.to_string_lossy().to_lowercase().contains(r"\target\"))
        .unwrap_or(false);
    let id = if installed { app.config().identifier.clone() } else { Toast::POWERSHELL_APP_ID.to_string() };
    let handle = app.clone();
    let _ = Toast::new(&id)
        .title(title)
        .text1(detail)
        .sound(None)
        .on_activated(move |_| {
            open_main(&handle, Some(screen));
            Ok(())
        })
        .show();
}

#[cfg(not(windows))]
pub fn toast(_app: &AppHandle, _title: &str, _detail: &str, _screen: Screen) {}

fn deliver(app: &AppHandle, r: &Reminder) {
    toast(app, &r.title, &r.detail, screen_of(&r.action));
}

/// "Back up now" from the tray.
pub fn backup_now(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let db_path = app.state::<AppState>().db_path.clone();
        let Ok(mut store) = SqliteStore::open(&db_path) else { return };
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

/// Starts the engine thread. `rx` receives file-system events and wake-ups.
pub fn spawn(app: AppHandle, rx: Receiver<EngineMsg>) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let db_path = state.db_path.clone();
        let mut store = match SqliteStore::open(&db_path) {
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

        // Volumes for the space recorder, refreshed each slow tick.
        let vols: Arc<Mutex<Vec<(String, i64, i64)>>> = Arc::default();
        let vols_for_watch = vols.clone();
        let mut watches: Vec<Box<dyn aegis_core::engine::Watch>> = vec![
            Box::new(OnlyCopyWatch),
            Box::new(QuarantineExpiryWatch),
            Box::new(DriveErrorsWatch),
            Box::new(SelfCheckWatch::new(vec![("live_index", LIVE_INDEX_EVERY_MS)], Some(db_path.clone()))),
            Box::new(SpaceWatch {
                volumes: Box::new(move || vols_for_watch.lock().map(|v| v.clone()).unwrap_or_default()),
                today: Box::new(|| Local::now().format("%Y-%m-%d").to_string()),
            }),
        ];
        if let Some(dl) = aegis_core::scanner::downloads_dir() {
            watches.push(Box::new(JanitorWatch { downloads: dl, installed: Box::new(installed_programs) }));
        }
        let mut engine = Engine::new(watches);

        let mut roots = enabled_roots(&store);
        let mut watcher = start_watcher(&app, &roots);
        let mut tripwire = Tripwire::new(roots.iter().map(|r| r.0.clone()).collect());
        let mut batcher = ChangeBatcher::new();
        let mut pending = false;
        let mut last_slow = std::time::Instant::now() - SLOW_TICK;

        loop {
            let timeout = if pending { FAST_TICK } else { SLOW_TICK };
            let mut woke = false;
            match rx.recv_timeout(timeout) {
                Ok(EngineMsg::Fs(ev)) => {
                    let now = now_ms();
                    for (path, kind) in change_kind(&ev) {
                        let shown = display(&path);
                        batcher.push(shown.clone(), now);
                        if let ChangeKind::Renamed { to } = &kind {
                            batcher.push(display(to), now);
                        }
                        let kind = match kind {
                            ChangeKind::Renamed { to } => ChangeKind::Renamed { to: display(&to) },
                            k => k,
                        };
                        tripwire.observe(&shown, kind, now);
                    }
                    pending = true;
                }
                Ok(EngineMsg::Wake) => woke = true,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            let now = now_ms();

            // Live index: apply quiet bursts in small transactions, grouped by volume.
            let chunks = batcher.drain_ready(now);
            if !chunks.is_empty() {
                pending = false;
                for chunk in chunks {
                    for (root, vol) in &roots {
                        let mine: Vec<PathBuf> =
                            chunk.iter().filter(|p| path_is_within(&p.to_string_lossy(), root, false)).cloned().collect();
                        if !mine.is_empty() {
                            let _ = index_changed(&mut store, vol, vec![mine]);
                        }
                    }
                }
            }

            // Tripwire: lock the folder's backup and tell the owner at once.
            if let Ok(trips) = tripwire.evaluate(store.conn(), now) {
                for trip in trips {
                    let _ = aegis_core::backup::guard::lock_folder(&mut store, &trip.root, "tripwire");
                    let f = trip_finding(&trip);
                    let _ = sync_findings(store.conn(), &[], std::slice::from_ref(&f), now);
                    let _ = app.emit(AEGIS_EVENT, AegisEvent::Tripped { v: 1, trip: trip.clone() });
                    woke = true;
                }
            }

            if !(woke || last_slow.elapsed() >= SLOW_TICK) {
                continue;
            }
            last_slow = std::time::Instant::now();

            // Roots may have changed (owner added a folder): re-watch.
            let now_roots = enabled_roots(&store);
            if now_roots != roots || watcher.is_none() {
                roots = now_roots;
                watcher = start_watcher(&app, &roots);
                tripwire = Tripwire::new(roots.iter().map(|r| r.0.clone()).collect());
            }
            if watcher.is_some() {
                let _ = record_run(store.conn(), "live_index", now, None);
            }

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
            let paused_until = state.paused_until.load(Ordering::SeqCst);
            let paused = paused_until > now;
            let mut changed = woke;
            if let Ok(report) = engine.tick(&mut store, now, &calm, paused) {
                changed |= !report.ran.is_empty();
            }

            // The weekly note.
            let local = local_now();
            let day = setting_u8(&store, "engine.note_day", 6);
            let hour = setting_u8(&store, "engine.note_hour", 18);
            if let Ok(Some(week)) = due_week(store.conn(), &local, day, hour)
                && let Ok(note) = build(store.conn(), &week, now - 7 * 86_400_000, &local.date, now)
                && save(store.conn(), &note, now).is_ok()
            {
                toast(&app, "Your weekly note is ready", &note.headline, Screen::Week);
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
            let backup_online =
                matches!(aegis_core::backup::backup_volume(store.conn()), Ok(Some((_, _, true))));
            if let Some(items) = state.tray_items.lock().ok().and_then(|g| g.as_ref().map(|i| i.backup_now.clone())) {
                let _ = items.set_enabled(backup_online);
            }
            if changed {
                let _ = app.emit(AEGIS_EVENT, AegisEvent::EngineUpdated { v: 1 });
            }
        }
    });
}

/// Watches the PC roots recursively; events go to the engine thread.
fn start_watcher(app: &AppHandle, roots: &[(String, String)]) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let tx = app.state::<AppState>().engine_tx.lock().ok()?.clone();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let _ = tx.send(EngineMsg::Fs(ev));
        }
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
}
