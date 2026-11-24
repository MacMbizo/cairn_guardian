//! Project 4 (System Care) integration tests: reclaimable cleanup & dead files, Policy Engine
//! `system_care` scope guards + 30-day quarantine/undo inside `TempDir`, `R3` app leftover
//! protections, uninstall & startup parsers (`installer_matches` regression), SMART health watch,
//! disk trends, and upgraded 5-dimension `health_breakdown`.

use aegis_core::care::apps::{
    is_safe_leftover_name, leftover_to_candidate, parse_uninstall_invocation, scan_leftover_folders,
};
use aegis_core::care::cleanup::{CleanupScanConfig, propose_care_quarantine, scan_cleanup};
use aegis_core::care::smart::parse_smart_json;
use aegis_core::care::startup::{make_impact_note_for_tests, parse_command_executable};
use aegis_core::care::trends::build_disk_trends;
use aegis_core::care::watch::SmartHealthWatch;
use aegis_core::care::{CleanupCategory, DriveHealthItem, InstalledApp, SYSTEM_CARE_AGENT};
use aegis_core::contracts::{ActionType, TargetKind};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, health_breakdown, rollback_batch};
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, normalize_path, upsert_discovered_file,
};
use aegis_core::engine::janitor::installer_matches;
use aegis_core::engine::note::date_of;
use aegis_core::engine::space::{day_index, record_day};
use aegis_core::engine::{Calm, Engine, Level, Screen};
use aegis_core::now_ms;
use aegis_core::policy::{PolicyAction, PolicyContext, PolicyVerdict, evaluate_policy};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

const DAY_MS: i64 = 86_400_000;

fn age_file(path: &Path, days_old: u64) {
    let target = SystemTime::now() - Duration::from_secs(days_old * 86_400);
    let f = OpenOptions::new().write(true).open(path).expect("open to set mtime");
    f.set_modified(target).expect("set_modified");
}

fn seed_temp_volume(store: &SqliteStore, id: &str, root: &Path, role: &str) {
    let mount = normalize_path(root, cfg!(not(windows))).0;
    let qdir = normalize_path(&root.join(".aegis-quarantine"), cfg!(not(windows))).0;
    store
        .conn()
        .execute(
            "INSERT OR REPLACE INTO volumes (
                id, mount_point, fs_type, is_removable, is_readonly, has_stable_inode,
                case_sensitive, quarantine_dir, role, status, last_seen
             ) VALUES (?1, ?2, 'ntfs', 0, 0, 1, 0, ?3, ?4, 'online', 1000)",
            params![id, mount, qdir, role],
        )
        .expect("seed volume");
}

fn seed_catalog_file(
    conn: &Connection,
    vol_id: &str,
    path: &Path,
    size: i64,
    mtime: i64,
    atomic_unit_id: Option<&str>,
) -> String {
    let raw = normalize_path(path, false).0;
    let idx = blake3::hash(raw.as_bytes()).as_bytes()[..16].to_vec();
    upsert_discovered_file(
        conn,
        &DiscoveredFileInput {
            volume_id: vol_id.to_string(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: Some(idx.clone()),
            nlink: 1,
            atomic_unit_id: atomic_unit_id.map(str::to_string),
            path: path.to_path_buf(),
            size,
            mtime,
            ctime: mtime,
            blake3: Some(idx),
            cloud_state: "local".to_string(),
        },
    )
    .expect("upsert catalog file")
}

#[test]
fn cleanup_scan_finds_temp_caches_dead_files_and_protects_projects_and_fresh_files() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    seed_temp_volume(&store, "v_tmp", dir.path(), "general");

    let temp_dir = dir.path().join("temp");
    let crash_dir = dir.path().join("crash");
    let browser_dir = dir.path().join("browser_cache");
    let dev_dir = dir.path().join("dev_cache");
    let proj_dir = dir.path().join("proj_root");
    let unit_dir = dir.path().join("unit_root");
    let loose_dir = dir.path().join("archives");
    for d in [&temp_dir, &crash_dir, &browser_dir, &dev_dir, &proj_dir, &unit_dir, &loose_dir] {
        fs::create_dir_all(d).unwrap();
    }

    let stale_tmp = temp_dir.join("old_setup.tmp");
    let fresh_tmp = temp_dir.join("active_build.tmp");
    let crash_dmp = crash_dir.join("app.dmp");
    let browser_entry = browser_dir.join("f_0001");
    let dev_wheel = dev_dir.join("numpy.whl");
    fs::write(&stale_tmp, b"old temp payload").unwrap();
    age_file(&stale_tmp, 3);
    fs::write(&fresh_tmp, b"fresh temp payload").unwrap();
    fs::write(&crash_dmp, b"dump bytes").unwrap();
    age_file(&crash_dmp, 5);
    fs::write(&browser_entry, vec![1u8; 4096]).unwrap();
    fs::write(&dev_wheel, vec![2u8; 4096]).unwrap();

    // Seed dead file candidates: 1 genuine dead large file, 1 inside project, 1 inside atomic unit, 1 recent.
    let dead_iso = loose_dir.join("ubuntu.iso");
    let proj_model = proj_dir.join("weights.bin");
    let unit_bundle = unit_dir.join("bundle.pak");
    let recent_zip = loose_dir.join("recent.zip");
    for p in [&dead_iso, &proj_model, &unit_bundle, &recent_zip] {
        fs::write(p, vec![7u8; 8192]).unwrap();
    }

    let now = now_ms();
    let old_ts = now - 200 * DAY_MS;
    let recent_ts = now - 10 * DAY_MS;

    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, computed_at)
             VALUES ('col_alpha', 'project', 'Alpha', 'software', 0.95, 'open', 1000)",
            [],
        )
        .unwrap();
    let proj_disp = normalize_path(&proj_dir, false).0;
    store
        .conn()
        .execute(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
             VALUES ('loc_alpha', 'col_alpha', 'v_tmp', ?1, 'home', 1, 8192, 1000, 1, 'hash', 0, 'fp')",
            params![proj_disp],
        )
        .unwrap();

    let unit_disp = normalize_path(&unit_dir, false).0;
    store
        .conn()
        .execute(
            "INSERT INTO atomic_units (id, volume_id, root_path, unit_type, marker_file, project_id, allow_split_override, created_at, updated_at)
             VALUES ('au_1', 'v_tmp', ?1, 'git', '.git', NULL, 0, 1000, 1000)",
            params![unit_disp],
        )
        .unwrap();

    let dead_fid = seed_catalog_file(store.conn(), "v_tmp", &dead_iso, 8192, old_ts, None);
    seed_catalog_file(store.conn(), "v_tmp", &proj_model, 8192, old_ts, None);
    seed_catalog_file(store.conn(), "v_tmp", &unit_bundle, 8192, old_ts, Some("au_1"));
    seed_catalog_file(store.conn(), "v_tmp", &recent_zip, 8192, recent_ts, None);

    let cfg = CleanupScanConfig {
        temp_dirs: vec![temp_dir],
        crash_dirs: vec![crash_dir],
        browser_cache_dirs: vec![("Edge Cache".into(), browser_dir)],
        dev_cache_dirs: vec![("pip cache".into(), dev_dir)],
        downloads_dir: None,
        installed_names: vec![],
        now_ms: now,
        dead_file_min_bytes: 5_000,
        dead_file_age_days: 180,
    };

    let rep = scan_cleanup(store.conn(), &cfg).unwrap();
    let find_cat = |cat: CleanupCategory| rep.groups.iter().find(|g| g.category == cat);

    let temps = find_cat(CleanupCategory::TempFiles).expect("TempFiles group");
    assert_eq!(temps.items.len(), 1);
    assert!(temps.items[0].path.ends_with("old_setup.tmp"), "fresh temp must be excluded");

    assert_eq!(find_cat(CleanupCategory::CrashDumps).unwrap().items.len(), 1);
    assert_eq!(find_cat(CleanupCategory::BrowserCache).unwrap().items.len(), 1);
    assert_eq!(find_cat(CleanupCategory::DevCache).unwrap().items.len(), 1);

    let dead = find_cat(CleanupCategory::DeadFiles).expect("DeadFiles group");
    assert_eq!(dead.items.len(), 1);
    assert_eq!(dead.items[0].file_id.as_deref(), Some(dead_fid.as_str()));
    assert!(dead.items[0].path.ends_with("ubuntu.iso"));
}

#[test]
fn care_quarantine_batch_executes_and_undoes_cleanly_while_policy_blocks_unsafe_paths() {
    let dir = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_temp_volume(&store, "v_tmp", dir.path(), "general");

    let temp_dir = dir.path().join("temp_work");
    let browser_cache_dir = dir.path().join("browser_cache");
    let leftover_parent = dir.path().join("local_appdata");
    let leftover_old = leftover_parent.join("RemovedOldSuite");
    let protected_vendor = leftover_parent.join("Microsoft");
    let aegis_data = temp_dir.join("com.aegis.guardian");
    for d in [&temp_dir, &browser_cache_dir, &leftover_old, &protected_vendor, &aegis_data] {
        fs::create_dir_all(d).unwrap();
    }

    let stale_file = temp_dir.join("old.tmp");
    let cache_blob = browser_cache_dir.join("data.bin");
    let fake_db = aegis_data.join("aegis.db");
    fs::write(&stale_file, b"temp contents 123").unwrap();
    age_file(&stale_file, 3);
    fs::write(&cache_blob, vec![9u8; 2048]).unwrap();
    fs::write(&fake_db, b"do not touch").unwrap();

    // 1. Verify PolicyContext scope guards cleanly with empty active_roots.
    let qdir = dir.path().join(".aegis-quarantine");
    let ctx = PolicyContext {
        mvp_mode: true,
        active_roots: vec![],
        volume_quarantine_dirs: vec![qdir],
        dials: HashMap::new(),
        care_cleanup_roots: vec![temp_dir.clone()],
        care_disposable_dirs: vec![browser_cache_dir.clone()],
        care_leftover_parents: vec![leftover_parent.clone()],
    };

    fn base_act<'a>(path: &'a Path, kind: TargetKind, agent: Option<&'a str>) -> PolicyAction<'a> {
        PolicyAction {
            action_type: ActionType::Quarantine,
            target_kind: kind,
            agent,
            from_path: path,
            to_path: None,
            confidence: 0.95,
            atomic_unit_root: None,
            unit_allow_split_override: false,
            file_unit_split_override: false,
            is_registered_cache_dir: false,
        }
    }

    // Allowed targets always require owner approval (`K1`).
    assert_eq!(
        evaluate_policy(&base_act(&stale_file, TargetKind::File, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::RequireApproval("quarantine_never_auto")
    );
    assert_eq!(
        evaluate_policy(&base_act(&browser_cache_dir, TargetKind::Dir, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::RequireApproval("quarantine_never_auto")
    );
    assert_eq!(
        evaluate_policy(&base_act(&leftover_old, TargetKind::Dir, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::RequireApproval("quarantine_never_auto")
    );

    // Unsafe targets must be blocked even for `system_care`.
    assert_eq!(
        evaluate_policy(&base_act(&temp_dir, TargetKind::Dir, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::Block("outside_configured_roots"),
        "cleanup root folder itself must never be quarantined"
    );
    assert_eq!(
        evaluate_policy(&base_act(Path::new(r"C:\Windows\System32\notepad.exe"), TargetKind::File, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::Block("outside_configured_roots")
    );
    assert_eq!(
        evaluate_policy(&base_act(&fake_db, TargetKind::File, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::Block("outside_configured_roots"),
        "com.aegis.guardian internals must never be allowed"
    );
    assert_eq!(
        evaluate_policy(&base_act(&protected_vendor, TargetKind::Dir, Some(SYSTEM_CARE_AGENT)), &ctx),
        PolicyVerdict::Block("outside_configured_roots"),
        "protected vendor directories must be blocked"
    );
    assert_eq!(
        evaluate_policy(&base_act(&stale_file, TargetKind::File, Some("untrusted_agent")), &ctx),
        PolicyVerdict::Block("outside_configured_roots")
    );

    // 2. End-to-end batch proposal, approval, quarantine execution inside TempDir, and full undo.
    let cfg = CleanupScanConfig {
        temp_dirs: vec![temp_dir],
        crash_dirs: vec![],
        browser_cache_dirs: vec![("TestBrowser".into(), browser_cache_dir)],
        dev_cache_dirs: vec![],
        downloads_dir: None,
        installed_names: vec![],
        now_ms: now_ms(),
        dead_file_min_bytes: 1_000_000,
        dead_file_age_days: 180,
    };
    let report = scan_cleanup(store.conn(), &cfg).unwrap();
    let items: Vec<_> = report.groups.into_iter().flat_map(|g| g.items).collect();
    assert_eq!(items.len(), 2);

    let batch_id = propose_care_quarantine(&mut store, &items).unwrap();
    approve_batch(&mut store, &batch_id, None).unwrap();
    let exec_res = execute_batch(&mut store, &batch_id).unwrap();
    assert_eq!(exec_res.executed_count, 2);
    assert_eq!(exec_res.failed_count, 0);
    assert!(!stale_file.exists());
    assert!(!cache_blob.exists());
    assert!(dir.path().join(".aegis-quarantine").join(&batch_id).exists());

    let rb = rollback_batch(&mut store, &batch_id).unwrap();
    assert_eq!(rb.reverted_count, 2);
    assert!(rb.conflicts.is_empty());
    assert_eq!(fs::read(&stale_file).unwrap(), b"temp contents 123");
    assert_eq!(fs::read(&cache_blob).unwrap().len(), 2048);
}

#[test]
fn leftover_folder_scanner_respects_r3_protections_and_age_thresholds() {
    let dir = TempDir::new().unwrap();
    let local_app = dir.path().join("AppDataLocal");
    let prog_files = dir.path().join("ProgramFiles");
    fs::create_dir_all(&local_app).unwrap();
    fs::create_dir_all(prog_files.join("Acme Toolset")).unwrap();

    let make_aged_folder = |name: &str, size: usize, age_days: u64| {
        let f = local_app.join(name);
        fs::create_dir_all(&f).unwrap();
        let file = f.join("cache.dat");
        fs::write(&file, vec![3u8; size]).unwrap();
        age_file(&file, age_days);
    };

    make_aged_folder("Microsoft", 2048, 90);
    make_aged_folder("Google", 2048, 90);
    make_aged_folder("com.aegis.guardian", 2048, 90);
    make_aged_folder("AcmeMedia", 2048, 90); // Matches installed app "Acme Media Suite"
    make_aged_folder("RecentNewApp", 2048, 5); // Too fresh (<30 days)
    make_aged_folder("SmallStaleApp", 256, 90); // Too small (<min_bytes)
    make_aged_folder("RemovedOldSuite", 4096, 60); // True leftover

    assert!(!is_safe_leftover_name("Microsoft"));
    assert!(!is_safe_leftover_name("com.aegis.guardian"));
    assert!(is_safe_leftover_name("RemovedOldSuite"));

    let installed = vec![InstalledApp {
        id: "hklm64:acme".into(),
        name: "Acme Media Suite".into(),
        publisher: Some("Acme Corp".into()),
        version: Some("1.0".into()),
        install_date: None,
        install_location: None,
        estimated_bytes: 50_000,
        has_uninstaller: true,
        last_used_ms: None,
        days_since_used: None,
        unused_likely: false,
        scope: "Machine".into(),
    }];

    let found = scan_leftover_folders(
        &[("AppData\\Local".into(), local_app)],
        &installed,
        &[prog_files],
        now_ms(),
        30,
        1024,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].folder_name, "RemovedOldSuite");
    let cand = leftover_to_candidate(&found[0]);
    assert_eq!(cand.category, CleanupCategory::UninstallLeftover);
    assert_eq!(cand.target_kind, TargetKind::Dir);
}

#[test]
fn uninstall_and_startup_parsers_handle_quoted_env_and_orphaned_commands() {
    let msi = parse_uninstall_invocation("MsiExec.exe /X{1111-2222} /qn").unwrap();
    assert_eq!(msi.0, "MsiExec.exe");
    assert_eq!(msi.1, vec!["/X{1111-2222}"]);

    let quoted = parse_uninstall_invocation(r#""C:\Program Files\Demo\unins000.exe" /SILENT"#).unwrap();
    assert_eq!(quoted.0, r"C:\Program Files\Demo\unins000.exe");
    assert_eq!(quoted.1, vec!["/SILENT"]);

    let dir = TempDir::new().unwrap();
    let exe_path = dir.path().join("agent.exe");
    fs::write(&exe_path, b"MZ").unwrap();
    let cmd = format!("\"{}\" --tray", exe_path.display());
    assert_eq!(parse_command_executable(&cmd), Some(exe_path));

    let env_cmd = r#""%TEMP%\definitely_missing_helper_12345.exe" --bg"#;
    let expanded = parse_command_executable(env_cmd).unwrap();
    assert!(!expanded.to_string_lossy().contains("%TEMP%"));

    let entry = make_impact_note_for_tests("User Run (HKCU)", r"C:\NoSuch\tool.exe --min", false);
    assert!(entry.orphaned);
    assert!(entry.impact_hint.contains("Orphaned"));

    // Installer matcher regression fix (`3+ word` installers no longer match on 1st word alone).
    assert!(!installer_matches(
        "VisualStudioCodeSetup-x64-1.95.0",
        &["Microsoft Visual C++ 2015-2022 Redistributable (x64)".into(), "Visual C++ Redistributable".into()]
    ));
    assert!(installer_matches(
        "VisualStudioCodeSetup-x64-1.95.0",
        &["Microsoft Visual Studio Code".into(), "Visual Studio Code".into()]
    ));
}

#[test]
fn smart_json_parser_and_watch_alert_only_on_real_degradation() {
    let healthy_single = r#"{
        "DeviceId": "0",
        "FriendlyName": "SAMSUNG MZVL21T0HDLU-00BT7",
        "MediaType": "SSD",
        "BusType": "NVMe",
        "Size": 1024209543168,
        "HealthStatus": "Healthy",
        "OperationalStatus": "OK",
        "DriveLetters": ["C:"],
        "Temperature": null,
        "Wear": null,
        "PowerOnHours": null,
        "ReadErrorsTotal": null,
        "WriteErrorsTotal": null,
        "ReliabilityError": "Access to a CIM resource was not available"
    }"#;
    let mut cat_errs = HashMap::new();
    cat_errs.insert("c:".to_string(), 2usize);
    let parsed = parse_smart_json(healthy_single, &cat_errs);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].health_status, "Healthy");
    assert_eq!(parsed[0].drive_letters, vec!["C:"]);
    assert_eq!(parsed[0].catalog_read_errors, 2);
    assert!(!parsed[0].smart_detail_available);
    assert!(parsed[0].smart_note.as_deref().unwrap().contains("Access to a CIM resource"));

    // Watch remains silent for healthy drives.
    let mut store = SqliteStore::open_in_memory().unwrap();
    let healthy_clone = parsed.clone();
    let mut engine = Engine::new(vec![Box::new(SmartHealthWatch {
        probe: Box::new(move |_| healthy_clone.clone()),
    })]);
    let calm = Calm { idle_secs: 0, on_ac: true, battery_pct: Some(100), busy: false };
    engine.tick(&mut store, 1_000, &calm, false).unwrap();
    assert!(aegis_core::engine::reminders::list_open(store.conn()).unwrap().is_empty());

    // Elevated wear / temperature triggers a `Level::Now` `Screen::Care` reminder.
    let bad_drive = vec![DriveHealthItem {
        device_id: "1".into(),
        model: "Aged NVMe".into(),
        media_type: "SSD".into(),
        bus_type: "NVMe".into(),
        size_bytes: 512_000_000_000,
        health_status: "Warning".into(),
        operational_status: "Predictive Failure".into(),
        drive_letters: vec!["D:".into()],
        temperature_c: Some(74),
        wear_pct: Some(92),
        power_on_hours: Some(25_000),
        read_errors: Some(4),
        write_errors: Some(0),
        smart_detail_available: true,
        smart_note: None,
        catalog_read_errors: 0,
    }];
    let mut alert_engine = Engine::new(vec![Box::new(SmartHealthWatch {
        probe: Box::new(move |_| bad_drive.clone()),
    })]);
    alert_engine.tick(&mut store, 1_000 + 6 * 3_600_000 + 1, &calm, false).unwrap();
    let rems = aegis_core::engine::reminders::list_open(store.conn()).unwrap();
    assert_eq!(rems.len(), 1);
    assert_eq!(rems[0].kind, "drive_smart");
    assert_eq!(rems[0].level, Level::Now);
    if let aegis_core::engine::FindingAction::Open { screen } = rems[0].action {
        assert_eq!(screen, Screen::Care);
    } else {
        panic!("expected Open {{ screen: Screen::Care }}");
    }
}

#[test]
fn disk_trends_and_upgraded_health_breakdown_compute_from_space_samples() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let dir = TempDir::new().unwrap();
    seed_temp_volume(&store, "v1", dir.path(), "general");

    let end_day = "2026-11-24";
    let end_idx = day_index(end_day).unwrap();
    let total = 100_000_000_000i64;
    for offset in (0..=30).rev() {
        let day = date_of(end_idx - offset);
        let progress = 30 - offset;
        let used = 70_000_000_000i64 + progress * 600_000_000i64; // 600 MB/day -> ~12w left
        let dl_bytes = 5_000_000_000i64 + progress * 200_000_000i64;
        record_day(
            store.conn(),
            &day,
            &[("v1".into(), used, total)],
            &[(r"C:\Users\Owner\Downloads".into(), dl_bytes)],
        )
        .unwrap();
    }

    let trends = build_disk_trends(&store, end_day, 30).unwrap();
    assert_eq!(trends.volumes.len(), 1);
    assert!(trends.volumes[0].bytes_per_week > 4_000_000_000);
    assert!(trends.volumes[0].weeks_left.is_some_and(|w| w < 10.0));
    assert_eq!(trends.roots.len(), 1);
    assert_eq!(trends.roots[0].name, "Downloads");
    assert_eq!(trends.roots[0].delta_7d, 7 * 200_000_000);
    assert_eq!(trends.roots[0].delta_30d, 30 * 200_000_000);

    // Health breakdown includes `storage`, `backup`, `duplication`, `caches`, `organization`.
    let before = health_breakdown(store.conn()).unwrap();
    assert_eq!(
        before.dimensions.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["storage", "backup", "duplication", "caches", "organization"]
    );

    // Seed a dead large file, duplicate pair, active cache_dir, and open drive_smart alert -> drops score.
    let f1 = dir.path().join("dup1.bin");
    let f2 = dir.path().join("dup2.bin");
    let dead = dir.path().join("dead.iso");
    for p in [&f1, &f2, &dead] {
        fs::write(p, b"abc").unwrap();
    }
    let old_ts = now_ms() - 200 * DAY_MS;
    let same_hash = [7u8; 32].to_vec();
    for (p, sz, h) in [(&f1, 1024, same_hash.clone()), (&f2, 1024, same_hash), (&dead, 150 * 1024 * 1024, [8u8; 32].to_vec())] {
        let disp = normalize_path(p, false).0;
        let idx = blake3::hash(disp.as_bytes()).as_bytes()[..16].to_vec();
        let fid = upsert_discovered_file(
            store.conn(),
            &DiscoveredFileInput {
                volume_id: "v1".into(),
                has_stable_inode: true,
                case_sensitive: false,
                file_index: Some(idx),
                nlink: 1,
                atomic_unit_id: None,
                path: p.to_path_buf(),
                size: sz,
                mtime: old_ts,
                ctime: old_ts,
                blake3: Some(h),
                cloud_state: "local".into(),
            },
        )
        .unwrap();
        insert_chunk(store.conn(), &fid, 0, "sample", 1, true).unwrap();
    }

    let mut engine = Engine::new(vec![Box::new(SmartHealthWatch {
        probe: Box::new(move |_| {
            vec![DriveHealthItem {
                device_id: "0".into(),
                model: "WarnDrive".into(),
                media_type: "SSD".into(),
                bus_type: "NVMe".into(),
                size_bytes: total,
                health_status: "Warning".into(),
                operational_status: "Degraded".into(),
                drive_letters: vec!["C:".into()],
                temperature_c: None,
                wear_pct: None,
                power_on_hours: None,
                read_errors: None,
                write_errors: None,
                smart_detail_available: false,
                smart_note: None,
                catalog_read_errors: 0,
            }]
        }),
    })]);
    let calm = Calm { idle_secs: 0, on_ac: true, battery_pct: Some(100), busy: false };
    engine.tick(&mut store, 1_000, &calm, false).unwrap();

    let after = health_breakdown(store.conn()).unwrap();
    assert!(after.overall_score < before.overall_score);
    let storage = after.dimensions.iter().find(|d| d.id == "storage").unwrap();
    assert!(storage.offenders_count >= 1);
    let caches = after.dimensions.iter().find(|d| d.id == "caches").unwrap();
    assert_eq!(caches.offenders_count, 1);
    let dupes = after.dimensions.iter().find(|d| d.id == "duplication").unwrap();
    assert_eq!(dupes.offenders_count, 1);
}
