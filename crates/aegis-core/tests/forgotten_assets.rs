use std::fs;
use std::path::Path;

use aegis_core::assets::{
    AssetFamily, AssetRecoverItem, AssetScanOptions, ForgottenReason, dismiss_forgotten_asset,
    enrich_asset_metadata, propose_asset_recovery, scan_forgotten_assets,
    undismiss_forgotten_asset,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, rollback_batch, simulate_batch};
use aegis_core::db::files::{
    DiscoveredFileInput, SENSITIVE_MASK, insert_chunk, set_file_sensitivity,
    soft_delete_file_in_tx, upsert_discovered_file,
};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use rusqlite::params;
use tempfile::TempDir;

const DAY_MS: i64 = 86_400_000;

fn seed_volume(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    status: &str,
    role: Option<&str>,
) {
    let meta = inspect_volume_for_path(root).unwrap();
    let qdir = root.join(".aegis-quarantine");
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, label, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, role, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, role = excluded.role",
            params![
                vol_id,
                root.to_string_lossy().to_string(),
                format!("Drive-{vol_id}"),
                meta.fs_type,
                i32::from(meta.has_stable_inode),
                i32::from(meta.case_sensitive),
                qdir.to_string_lossy().to_string(),
                status,
                role.unwrap_or("general"),
                aegis_core::now_ms(),
            ],
        )
        .unwrap();
}

fn seed_file_with_age(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    rel_path: &str,
    bytes: &[u8],
    mtime_ms: i64,
) -> String {
    let file_path = root.join(rel_path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, bytes).unwrap();
    let st = inspect_path_metadata(&file_path).unwrap().unwrap();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: None,
        path: file_path,
        size: bytes.len() as i64,
        mtime: mtime_ms,
        ctime: mtime_ms,
        blake3: Some(blake3::hash(bytes).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    let fid = upsert_discovered_file(store.conn(), &input).unwrap();
    store
        .conn()
        .execute("UPDATE files SET mtime = ?1, size = ?2 WHERE id = ?3", params![mtime_ms, bytes.len() as i64, fid])
        .unwrap();
    fid
}

fn seed_collection(
    store: &SqliteStore,
    col_id: &str,
    kind: &str,
    name: &str,
    project_kind: Option<&str>,
) {
    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at)
             VALUES (?1, ?2, ?3, ?4, 0.95, 'confirmed', '[]', '[]', ?5)",
            params![col_id, kind, name, project_kind, aegis_core::now_ms()],
        )
        .unwrap();
}

#[test]
fn assets_buried_in_clutter_match_collections_and_suggest_destination() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    // Active design collection with online member inside Projects\SmoothLite.
    let organized_id = seed_file_with_age(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\SmoothLite\\smoothlite_cover.psd",
        b"8BPS\x00\x01smoothlite-cover",
        now - 200 * DAY_MS,
    );
    seed_collection(&store, "col-smooth", "design_set", "SmoothLite", Some("design"));
    let organized_path: String = store
        .conn()
        .query_row("SELECT path FROM files WHERE id = ?1", params![organized_id], |r| r.get(0))
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_members (collection_id, file_id, volume_id, path, size, mtime)
             VALUES ('col-smooth', ?1, 'v-main', ?2, 24, ?3)",
            params![organized_id, organized_path, now - 200 * DAY_MS],
        )
        .unwrap();

    // Stranded asset buried in Downloads matching SmoothLite.
    let stranded_id = seed_file_with_age(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\SmoothLite_Hero_Banner.psd",
        b"8BPS\x00\x01stranded-banner",
        now - 140 * DAY_MS,
    );

    // Recent unrelated download (< 90d) should NOT appear in default 90d scan.
    let _recent_id = seed_file_with_age(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\Today_Notes.pdf",
        &vec![b'a'; 12_000],
        now - 5 * DAY_MS,
    );

    let rep = scan_forgotten_assets(store.conn(), &AssetScanOptions::default(), None, now, false)
        .unwrap();
    assert_eq!(rep.total_count, 1);
    assert_eq!(rep.stranded_count, 1);
    assert_eq!(rep.buried_count, 1);
    let hit = &rep.items[0];
    assert_eq!(hit.file_id, stranded_id);
    assert_eq!(hit.family, AssetFamily::DesignSource);
    assert!(hit.reasons.contains(&ForgottenReason::BuriedInClutter));
    assert!(hit.reasons.contains(&ForgottenReason::StrandedFromCollection));
    assert_eq!(hit.matched_collection_name.as_deref(), Some("SmoothLite"));
    assert!(
        hit.suggested_destination
            .as_deref()
            .is_some_and(|d| d.ends_with("SmoothLite")),
        "expected SmoothLite dest, got {:?}",
        hit.suggested_destination
    );
}

#[test]
fn assets_on_offline_drive_detect_only_copy_and_skip_redundant_online_mirrors() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    let online_root = tmp.path().join("online");
    let offline_root = tmp.path().join("offline");
    fs::create_dir_all(&online_root).unwrap();
    fs::create_dir_all(&offline_root).unwrap();

    seed_volume(&store, "v-on", &online_root, "online", None);
    seed_volume(&store, "v-off", &offline_root, "offline", None);

    // Online project file + mirror in an archived offline project with identical bytes.
    let shared_bytes = vec![0x7Au8; 16_000];
    let on_id = seed_file_with_age(
        &store,
        "v-on",
        &online_root,
        "Projects\\Brand\\brand_master.png",
        &shared_bytes,
        now - 10 * DAY_MS,
    );
    let off_dup_id = seed_file_with_age(
        &store,
        "v-off",
        &offline_root,
        "Archived\\Brand\\brand_master.png",
        &shared_bytes,
        now - 200 * DAY_MS,
    );
    seed_collection(&store, "col-brand", "project", "Brand", Some("design"));
    for (fid, vid, p) in [
        (&on_id, "v-on", online_root.join("Projects\\Brand")),
        (&off_dup_id, "v-off", offline_root.join("Archived\\Brand")),
    ] {
        store
            .conn()
            .execute(
                "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
                 VALUES (?1, 'col-brand', ?2, ?3, 'same', 1, 16000, ?4, 1, 'contents', 0, ?5)",
                params![
                    format!("loc-{fid}"),
                    vid,
                    p.to_string_lossy().to_string(),
                    now,
                    format!("fp-{fid}")
                ],
            )
            .unwrap();
    }

    // Unique RAW asset sitting on unplugged offline drive.
    let raw_id = seed_file_with_age(
        &store,
        "v-off",
        &offline_root,
        "Exports\\Studio_0042.NEF",
        b"RAW-UNIQUE-NEF-BYTES",
        now - 320 * DAY_MS,
    );

    let rep = scan_forgotten_assets(
        store.conn(),
        &AssetScanOptions {
            drive_status: Some("offline".into()),
            ..Default::default()
        },
        None,
        now,
        false,
    )
    .unwrap();
    assert_eq!(rep.total_count, 1);
    assert_eq!(rep.offline_count, 1);
    assert_eq!(rep.only_copy_count, 1);
    assert_eq!(rep.items[0].file_id, raw_id);
    assert!(!rep.items[0].drive_online);
    assert!(rep.items[0].only_copy);
    assert!(rep.items[0].reasons.contains(&ForgottenReason::OfflineDrive));
}

#[test]
fn assets_respect_k2_quarantine_k4_backup_exclusion_and_atomic_units() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    let main_root = tmp.path().join("main");
    let bak_root = tmp.path().join("bak");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&bak_root).unwrap();

    seed_volume(&store, "v-main", &main_root, "online", None);
    seed_volume(&store, "v-bak", &bak_root, "online", Some("backup"));

    let q_id = seed_file_with_age(&store, "v-main", &main_root, "q.psd", b"8BPSq", now - 200 * DAY_MS);
    let del_id = seed_file_with_age(&store, "v-main", &main_root, "d.psd", b"8BPSd", now - 200 * DAY_MS);
    let _bak_id = seed_file_with_age(&store, "v-bak", &bak_root, "bak.psd", b"8BPSb", now - 200 * DAY_MS);
    let unit_file = seed_file_with_age(&store, "v-main", &main_root, "Repo\\logo.png", &vec![1u8; 12_000], now - 200 * DAY_MS);
    let _tiny_png = seed_file_with_age(&store, "v-main", &main_root, "thumb.png", &[2u8; 1200], now - 200 * DAY_MS);
    let svg_id = seed_file_with_age(&store, "v-main", &main_root, "vector.svg", b"<svg></svg>", now - 200 * DAY_MS);

    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![q_id])
        .unwrap();
    let tx = store.conn_mut().transaction().unwrap();
    soft_delete_file_in_tx(&tx, &del_id).unwrap();
    tx.commit().unwrap();

    store
        .conn()
        .execute(
            "INSERT INTO atomic_units (id, volume_id, root_path, unit_type, marker_file, allow_split_override, created_at, updated_at)
             VALUES ('au-1', 'v-main', ?1, 'git_repo', '.git', 0, ?2, ?2)",
            params![main_root.join("Repo").to_string_lossy().to_string(), now],
        )
        .unwrap();
    store
        .conn()
        .execute("UPDATE files SET atomic_unit_id = 'au-1' WHERE id = ?1", params![unit_file])
        .unwrap();

    let rep = scan_forgotten_assets(store.conn(), &AssetScanOptions::default(), None, now, false)
        .unwrap();
    assert_eq!(rep.total_count, 1);
    assert_eq!(rep.items[0].file_id, svg_id);
}

#[test]
fn assets_enforce_r7_sensitive_masking_and_persist_dismissals() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    let sec_id = seed_file_with_age(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\Executive_Comp_Plan.pdf",
        &vec![b'%'; 12_000],
        now - 150 * DAY_MS,
    );
    insert_chunk(store.conn(), &sec_id, 0, "confidential salary target 99500", 5, true).unwrap();
    set_file_sensitivity(store.conn(), &sec_id, "confidential").unwrap();

    // Locked sensitive session: snippet is masked and hidden text cannot be queried.
    let locked_rep =
        scan_forgotten_assets(store.conn(), &AssetScanOptions::default(), None, now, false).unwrap();
    assert_eq!(locked_rep.items.len(), 1);
    assert_eq!(locked_rep.items[0].snippet.as_deref(), Some(SENSITIVE_MASK));

    let leak_probe = scan_forgotten_assets(
        store.conn(),
        &AssetScanOptions {
            query: Some("99500".into()),
            ..Default::default()
        },
        None,
        now,
        false,
    )
    .unwrap();
    assert_eq!(leak_probe.total_count, 0);

    // Unlocked session reveals snippet and lets query match chunk text.
    let unlocked_probe = scan_forgotten_assets(
        store.conn(),
        &AssetScanOptions {
            query: Some("99500".into()),
            ..Default::default()
        },
        None,
        now,
        true,
    )
    .unwrap();
    assert_eq!(unlocked_probe.total_count, 1);
    assert!(unlocked_probe.items[0].snippet.as_deref().is_some_and(|s| s.contains("99500")));

    // Dismissal hides item from default scan and restores cleanly.
    dismiss_forgotten_asset(store.conn(), &sec_id, "keep_there").unwrap();
    let after_dismiss =
        scan_forgotten_assets(store.conn(), &AssetScanOptions::default(), None, now, false).unwrap();
    assert_eq!(after_dismiss.total_count, 0);
    assert_eq!(after_dismiss.dismissed_count, 1);

    let with_dismissed = scan_forgotten_assets(
        store.conn(),
        &AssetScanOptions {
            include_dismissed: Some(true),
            ..Default::default()
        },
        None,
        now,
        false,
    )
    .unwrap();
    assert_eq!(with_dismissed.total_count, 1);
    assert!(with_dismissed.items[0].dismissed);

    undismiss_forgotten_asset(store.conn(), &sec_id).unwrap();
    let restored =
        scan_forgotten_assets(store.conn(), &AssetScanOptions::default(), None, now, false).unwrap();
    assert_eq!(restored.total_count, 1);
    assert!(!restored.items[0].dismissed);
}

#[test]
fn assets_k1_recovery_propose_simulate_approve_execute_rollback_and_offline_reject() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    let on_root = tmp.path().join("main");
    let off_root = tmp.path().join("off");
    fs::create_dir_all(&on_root).unwrap();
    fs::create_dir_all(&off_root).unwrap();

    seed_volume(&store, "v-on", &on_root, "online", None);
    seed_volume(&store, "v-off", &off_root, "offline", None);
    store.add_root(&on_root).unwrap();

    let src_id = seed_file_with_age(
        &store,
        "v-on",
        &on_root,
        "Downloads\\Brand_Poster.png",
        &vec![0x42; 10_000],
        now - 120 * DAY_MS,
    );
    let off_id = seed_file_with_age(
        &store,
        "v-off",
        &off_root,
        "Archive\\Old.psd",
        b"8BPS-off",
        now - 200 * DAY_MS,
    );

    let dest_dir = on_root.join("Projects\\SmoothLite");
    fs::create_dir_all(&dest_dir).unwrap();

    // Offline files are rejected cleanly before creating any batch.
    let err = propose_asset_recovery(
        &mut store,
        &[AssetRecoverItem {
            file_id: off_id,
            dest_dir: dest_dir.to_string_lossy().to_string(),
            reason: None,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("offline"));

    // Propose -> simulate -> approve -> execute -> rollback (`K1`).
    let batch_id = propose_asset_recovery(
        &mut store,
        &[AssetRecoverItem {
            file_id: src_id,
            dest_dir: dest_dir.to_string_lossy().to_string(),
            reason: Some("Move Brand_Poster into SmoothLite".into()),
        }],
    )
    .unwrap();

    let sim = simulate_batch(&store, &batch_id).unwrap();
    assert_eq!(sim.blocked_count, 0);
    assert_eq!(sim.ready_count, 1);

    approve_batch(&mut store, &batch_id, None).unwrap();
    let exec = execute_batch(&mut store, &batch_id).unwrap();
    assert_eq!(exec.executed_count, 1);
    assert_eq!(exec.failed_count, 0);

    let src_path = on_root.join("Downloads\\Brand_Poster.png");
    let moved_path = dest_dir.join("Brand_Poster.png");
    assert!(!src_path.exists());
    assert!(moved_path.is_file());

    let rb = rollback_batch(&mut store, &batch_id).unwrap();
    assert_eq!(rb.reverted_count, 1);
    assert!(src_path.is_file());
    assert!(!moved_path.exists());
}

#[test]
fn assets_enrich_header_metadata_caches_dimensions_aspect_and_color_in_asset_meta() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-meta", tmp.path(), "online", None);

    let mut png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png_bytes.extend_from_slice(&1920u32.to_be_bytes());
    png_bytes.extend_from_slice(&1080u32.to_be_bytes());
    png_bytes.resize(10_000, 0);

    let svg_bytes =
        br##"<svg viewBox="0 0 800 600"><rect fill="#FFFFFF"/><circle fill="#10B981"/></svg>"##;

    let png_id = seed_file_with_age(&store, "v-meta", tmp.path(), "Downloads\\Hero_1080p.png", &png_bytes, now - 120 * DAY_MS);
    let svg_id = seed_file_with_age(&store, "v-meta", tmp.path(), "Downloads\\Mark.svg", svg_bytes, now - 120 * DAY_MS);

    let enriched = enrich_asset_metadata(store.conn(), &[png_id.clone(), svg_id.clone()]).unwrap();
    assert_eq!(enriched, 2);

    let rep = scan_forgotten_assets(store.conn(), &AssetScanOptions::default(), None, now, false)
        .unwrap();
    assert_eq!(rep.total_count, 2);

    let png_item = rep.items.iter().find(|i| i.file_id == png_id).unwrap();
    assert_eq!((png_item.width, png_item.height), (Some(1920), Some(1080)));
    assert_eq!(png_item.aspect_label.as_deref(), Some("16:9"));

    let svg_item = rep.items.iter().find(|i| i.file_id == svg_id).unwrap();
    assert_eq!((svg_item.width, svg_item.height), (Some(800), Some(600)));
    assert_eq!(svg_item.dominant_color.as_deref(), Some("#10B981"));
}
