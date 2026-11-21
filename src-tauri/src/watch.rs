//! Plug-in watcher: polls mounted drives while the app is open (spec §7.2, §7.4).
//!
//! On a change it syncs the drive registry, tells the UI, refreshes known drives, and, when
//! the Backup drive arrives with a standing approval, tops it up and prunes old versions.

use crate::commands::{AEGIS_EVENT, AppState};
use aegis_core::backup::topup::{TopUpAuthority, plan_topup, prune_previous_versions, run_topup};
use aegis_core::contracts::AegisEvent;
use aegis_core::db::SqliteStore;
use aegis_core::drives::{SyncReport, VolumeProvider, refresh_volume, sync_mounted};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// How often mounted drives are polled.
pub const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// `(arrived, departed)` between two snapshots of mounted drives.
pub fn diff_mounts<T: PartialEq + Clone>(prev: &[T], now: &[T]) -> (Vec<T>, Vec<T>) {
    let arrived = now.iter().filter(|m| !prev.contains(m)).cloned().collect();
    let departed = prev.iter().filter(|m| !now.contains(m)).cloned().collect();
    (arrived, departed)
}

fn provider() -> Box<dyn VolumeProvider> {
    #[cfg(windows)]
    {
        Box::new(aegis_core::drives::WindowsProvider)
    }
    #[cfg(not(windows))]
    {
        Box::new(aegis_core::drives::FakeVolumeProvider::new())
    }
}

fn emit(app: &AppHandle, ev: AegisEvent) {
    let _ = app.emit(AEGIS_EVENT, ev);
}

fn fail(app: &AppHandle, job: &str, err: impl std::fmt::Display) {
    emit(app, AegisEvent::JobFailed { v: 1, job: job.into(), message: err.to_string() });
}

/// Starts the watcher on its own thread with its own database connection.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let db_path = app.state::<AppState>().db_path.clone();
        let mut store = match SqliteStore::open(&db_path) {
            Ok(s) => s,
            Err(e) => return fail(&app, "drive_watch", e),
        };
        let provider = provider();
        let mut prev: Vec<DriveKey> = Vec::new();
        loop {
            // Keyed by identity, not just the letter: a different drive arriving at the same
            // letter between two polls is still a change.
            let now: Vec<DriveKey> = provider
                .mounted()
                .map(|v| {
                    v.into_iter()
                        .filter(|m| !m.is_system)
                        .map(|m| (m.mount_point, m.serial, m.guid_path, m.total_bytes))
                        .collect()
                })
                .unwrap_or_default();
            let (arrived, departed) = diff_mounts(&prev, &now);
            if !arrived.is_empty() || !departed.is_empty() {
                let seen_before = last_seen_map(&store);
                match sync_mounted(&mut store, provider.as_ref()) {
                    Ok(report) => handle(&app, &mut store, &report, &seen_before),
                    Err(e) => fail(&app, "drive_sync", e),
                }
            }
            prev = now;
            std::thread::sleep(POLL_INTERVAL);
        }
    });
}

/// `(mount point, serial, volume GUID, size)` of a mounted drive.
type DriveKey = (PathBuf, Option<String>, Option<String>, u64);

fn last_seen_map(store: &SqliteStore) -> HashMap<String, i64> {
    store
        .list_volumes()
        .map(|vs| vs.into_iter().map(|v| (v.id, v.last_seen)).collect())
        .unwrap_or_default()
}

fn handle(app: &AppHandle, store: &mut SqliteStore, report: &SyncReport, seen_before: &HashMap<String, i64>) {
    for id in &report.departed {
        emit(app, AegisEvent::DriveDeparted { v: 1, volume_id: id.clone() });
    }
    let volumes = store.list_volumes().unwrap_or_default();
    for id in &report.arrived {
        let Some(v) = volumes.iter().find(|v| &v.id == id) else { continue };
        let known = !report.new_drives.contains(id);
        emit(
            app,
            AegisEvent::DriveArrived {
                v: 1,
                volume_id: id.clone(),
                label: v.label.clone(),
                last_seen: seen_before.get(id).copied().unwrap_or(v.last_seen),
                known,
                role: v.role.clone(),
            },
        );
        if !known || v.last_catalog_complete_at.is_none() {
            // A brand-new drive waits for the owner's click; one whose catalog was started
            // before and interrupted picks up again (spec §6.7).
            let resume = aegis_core::engine::watches::catalogs_to_resume(store.conn()).unwrap_or_default();
            if resume.contains(id) {
                crate::commands::drives::start_catalog(app, id.clone());
            }
            continue;
        }
        // Refresh and top-up can take a long time: run them on a worker with its own
        // connection, so the watcher keeps noticing drives come and go meanwhile.
        let (app, id, is_backup) = (app.clone(), id.clone(), v.role == "backup");
        std::thread::spawn(move || {
            let db_path = app.state::<AppState>().db_path.clone();
            let mut store = match SqliteStore::open(&db_path) {
                Ok(s) => s,
                Err(e) => return fail(&app, "drive_refresh", e),
            };
            let cancel = std::sync::atomic::AtomicBool::new(false);
            match refresh_volume(&mut store, &id, &cancel) {
                Ok(r) => emit(&app, AegisEvent::DriveRefreshed { v: 1, report: r }),
                Err(e) => fail(&app, "drive_refresh", e),
            }
            if is_backup && aegis_core::policy::has_standing_topup(store.conn(), &id).unwrap_or(false) {
                standing_topup(&app, &mut store, &id);
            }
        });
    }
}

/// Runs an automatic top-up under the standing approval, then prunes expired versions.
pub fn standing_topup(app: &AppHandle, store: &mut SqliteStore, backup_id: &str) {
    run_topup_job(app, store, backup_id, TopUpAuthority::Standing);
}

/// Shared by the watcher and the `backup_topup_start` command.
pub fn run_topup_job(app: &AppHandle, store: &mut SqliteStore, backup_id: &str, authority: TopUpAuthority) {
    let state = app.state::<AppState>();
    if state.topup_running.swap(true, Ordering::SeqCst) {
        return; // one top-up at a time
    }
    state.topup_cancel.store(false, Ordering::SeqCst);
    let cancel = state.topup_cancel.clone();
    let result = (|| -> aegis_core::error::Result<()> {
        // See edits made since the last scan, so "up to date" is true.
        aegis_core::backup::topup::refresh_sources(store)?;
        let plan = plan_topup(store.conn(), backup_id)?;
        if !plan.halted_folders.is_empty() {
            emit(app, AegisEvent::GuardHalted {
                v: 1,
                backup_volume_id: backup_id.into(),
                halts: plan.halted_folders.clone(),
            });
        }
        let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
        let res = run_topup(store, &plan, authority, &cancel, &mut |done, total| {
            if last_emit.elapsed() >= Duration::from_millis(250) || done == total {
                last_emit = std::time::Instant::now();
                emit(app, AegisEvent::TopUpProgress {
                    v: 1,
                    backup_volume_id: backup_id.into(),
                    bytes_done: done,
                    bytes_total: total,
                });
            }
        })?;
        prune_previous_versions(store, backup_id, aegis_core::now_ms())?;
        emit(app, AegisEvent::TopUpFinished { v: 1, backup_volume_id: backup_id.into(), result: res });
        Ok(())
    })();
    state.topup_running.store(false, Ordering::SeqCst);
    if let Err(e) = result {
        fail(app, "backup_topup", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_diff_detects_arrival_and_departure() {
        let e = PathBuf::from(r"E:\");
        let f = PathBuf::from(r"F:\");
        let g = PathBuf::from(r"G:\");
        let (arrived, departed) = diff_mounts(&[e.clone(), f.clone()], &[f.clone(), g.clone()]);
        assert_eq!(arrived, vec![g]);
        assert_eq!(departed, vec![e]);
        let (a, d) = diff_mounts(std::slice::from_ref(&f), std::slice::from_ref(&f));
        assert!(a.is_empty() && d.is_empty());
    }

    /// Review C1: a different drive at the same letter between two polls is a change.
    #[test]
    fn swapped_drive_at_same_letter_is_a_change() {
        let e = PathBuf::from(r"E:\");
        let before: Vec<DriveKey> = vec![(e.clone(), Some("aaaa".into()), Some("guid-a".into()), 1)];
        let after: Vec<DriveKey> = vec![(e, Some("cccc".into()), Some("guid-c".into()), 1)];
        let (arrived, departed) = diff_mounts(&before, &after);
        assert_eq!((arrived.len(), departed.len()), (1, 1));
    }
}
