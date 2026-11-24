use std::fs;
use std::path::Path;

use aegis_core::cross_project::{
    CrossProjectOptions, DependencyKind, FindingKind, InsightStatus, prepare_cross_project_brief,
    scan_cross_project_intelligence, set_insight_status,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, set_file_sensitivity, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::db::vec::{PendingChunk, upsert_chunk_embeddings};
use aegis_core::error::AegisError;
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use rusqlite::params;
use tempfile::TempDir;

const DAY_MS: i64 = 86_400_000;

fn seed_volume(store: &SqliteStore, vol_id: &str, root: &Path, role: Option<&str>) {
    let meta = inspect_volume_for_path(root).unwrap();
    let qdir = root.join(".aegis-quarantine");
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, label, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, role, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'online', ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, role = excluded.role",
            params![
                vol_id,
                root.to_string_lossy().to_string(),
                format!("Drive-{vol_id}"),
                meta.fs_type,
                i32::from(meta.has_stable_inode),
                i32::from(meta.case_sensitive),
                qdir.to_string_lossy().to_string(),
                role.unwrap_or("general"),
                aegis_core::now_ms(),
            ],
        )
        .unwrap();
}

fn seed_collection(store: &SqliteStore, col_id: &str, name: &str, vol_id: &str, root: &Path) {
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at)
             VALUES (?1, 'project', ?2, 'software', 0.95, 'confirmed', '[]', '[]', ?3)",
            params![col_id, name, now],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
             VALUES (?1, ?2, ?3, ?4, 'home', 2, 1024, ?5, 0, 'hash', 0, ?6)",
            params![
                format!("loc-{col_id}"),
                col_id,
                vol_id,
                root.to_string_lossy().to_string(),
                now,
                format!("fp-{col_id}")
            ],
        )
        .unwrap();
}

fn seed_doc(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    rel_path: &str,
    text: &str,
    mtime_ms: i64,
    blake3_override: Option<[u8; 32]>,
) -> (String, i64) {
    let file_path = root.join(rel_path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let bytes = text.as_bytes();
    fs::write(&file_path, bytes).unwrap();
    let st = inspect_path_metadata(&file_path).unwrap().unwrap();
    let digest = blake3_override.unwrap_or_else(|| *blake3::hash(bytes).as_bytes());
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
        blake3: Some(digest.to_vec()),
        cloud_state: st.cloud_state,
    };
    let fid = upsert_discovered_file(store.conn(), &input).unwrap();
    store
        .conn()
        .execute(
            "UPDATE files SET mtime = ?1, size = ?2, blake3 = ?3 WHERE id = ?4",
            params![mtime_ms, bytes.len() as i64, digest.as_slice(), fid],
        )
        .unwrap();
    let tok = text.split_whitespace().count() as i64;
    let chunk_uuid = insert_chunk(store.conn(), &fid, 0, text, tok, true).unwrap();
    let cid: i64 = store
        .conn()
        .query_row(
            "SELECT id FROM chunks WHERE chunk_uuid = ?1",
            params![chunk_uuid],
            |r| r.get(0),
        )
        .unwrap();
    (fid, cid)
}

fn seed_entity(
    store: &SqliteStore,
    entity_id: &str,
    entity_type: &str,
    name: &str,
    attrs_json: &str,
    file_ids: &[&str],
) {
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT OR IGNORE INTO entities (id, entity_type, name, attrs_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![entity_id, entity_type, name, attrs_json, now],
        )
        .unwrap();
    for fid in file_ids {
        store
            .conn()
            .execute(
                "INSERT INTO relationships (id, src_id, src_kind, dst_id, dst_kind, rel_type, valid_from, created_at)
                 VALUES (?1, ?2, 'file', ?3, 'entity', 'mentions', ?4, ?4)",
                params![format!("rel-{entity_id}-{fid}"), fid, entity_id, now],
            )
            .unwrap();
    }
}

#[test]
fn detects_duplicate_effort_across_projects_via_vectors_and_exact_hash() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(
        &store,
        "col-alpha",
        "Project Alpha",
        "v-main",
        &tmp.path().join("Projects\\Alpha"),
    );
    seed_collection(
        &store,
        "col-beta",
        "Project Beta",
        "v-main",
        &tmp.path().join("Projects\\Beta"),
    );

    let (fa, ca) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Alpha\\benchmark_study.md",
        "Performance benchmark comparing sqlite storage throughput and cache hit ratio.",
        now - 21 * DAY_MS,
        Some([10u8; 32]),
    );
    let (fb, cb) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Beta\\latency_analysis.md",
        "Evaluates local persistence engine IO speeds across desktop SSD partitions.",
        now - 2 * DAY_MS,
        Some([20u8; 32]),
    );
    let (fa_exact, ca_exact) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Alpha\\shared_checklist.txt",
        "Release checklist identical hash copy.",
        now - 10 * DAY_MS,
        Some([99u8; 32]),
    );
    let (fb_exact, cb_exact) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Beta\\shared_checklist_copy.txt",
        "Release checklist identical hash copy with unique bytes override.",
        now - 3 * DAY_MS,
        Some([99u8; 32]),
    );

    upsert_chunk_embeddings(
        store.conn(),
        "test-embed",
        3,
        &[
            (
                PendingChunk {
                    chunk_id: ca,
                    chunk_uuid: "ua".into(),
                    file_id: fa.clone(),
                    embed_text: "a".into(),
                    content_hash: [1u8; 32],
                },
                vec![1.0, 0.0, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: cb,
                    chunk_uuid: "ub".into(),
                    file_id: fb.clone(),
                    embed_text: "b".into(),
                    content_hash: [2u8; 32],
                },
                vec![0.95, 0.312, 0.0], // cosine = 0.95
            ),
            (
                PendingChunk {
                    chunk_id: ca_exact,
                    chunk_uuid: "uae".into(),
                    file_id: fa_exact.clone(),
                    embed_text: "ae".into(),
                    content_hash: [3u8; 32],
                },
                vec![0.0, 1.0, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: cb_exact,
                    chunk_uuid: "ube".into(),
                    file_id: fb_exact.clone(),
                    embed_text: "be".into(),
                    content_hash: [4u8; 32],
                },
                vec![0.0, 0.0, 1.0],
            ),
        ],
    )
    .unwrap();

    let report =
        scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
            .unwrap();
    assert!(report.used_vectors);
    assert_eq!(report.total_projects, 2);
    assert_eq!(report.duplicate_effort_count, 2);

    let sem = report
        .findings
        .iter()
        .find(|f| f.file_a.as_ref().is_some_and(|x| x.file_id == fa))
        .expect("semantic duplicate finding missing");
    assert_eq!(sem.kind, FindingKind::DuplicateEffort);
    assert_eq!(sem.age_delta_days, 19);
    assert!(sem.summary.contains("19 day(s) earlier"));
    assert!(sem.similarity > 0.94);

    let exact = report
        .findings
        .iter()
        .find(|f| f.similarity == 1.0)
        .expect("exact hash duplicate finding missing");
    assert_eq!(exact.age_delta_days, 7);
}

#[test]
fn detects_conflicting_decisions_and_stack_version_divergence() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(
        &store,
        "col-helios",
        "Helios",
        "v-main",
        &tmp.path().join("Helios"),
    );
    seed_collection(
        &store,
        "col-atlas",
        "Atlas",
        "v-main",
        &tmp.path().join("Atlas"),
    );

    let (fh, _) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Helios\\architecture.md",
        "Decision notes for Helios cache layer.",
        now - 4 * DAY_MS,
        None,
    );
    let (fa, _) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Atlas\\architecture.md",
        "Decision notes for Atlas cache layer.",
        now - DAY_MS,
        None,
    );

    // Opposite polarity on telemetry caching + Postgres vs SQLite conflict.
    seed_entity(
        &store,
        "dec-h-1",
        "decision",
        "Adopt local telemetry cache layer",
        r#"{"summary":"Adopt local telemetry cache layer across workers"}"#,
        &[&fh],
    );
    seed_entity(
        &store,
        "dec-a-1",
        "decision",
        "Avoid telemetry cache layer in workers",
        r#"{"summary":"Disable local telemetry cache layer to keep memory minimal"}"#,
        &[&fa],
    );

    // Major dependency drift in manifests (react ^17 vs ^19, serde 1 vs 2 or pydantic).
    seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Helios\\package.json",
        r#"{"dependencies": {"react": "^17.0.2", "zustand": "^4.5.0"}}"#,
        now - 3 * DAY_MS,
        None,
    );
    seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Atlas\\package.json",
        r#"{"dependencies": {"react": "^19.0.0", "zustand": "^4.5.2"}}"#,
        now - DAY_MS,
        None,
    );

    let report =
        scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
            .unwrap();
    assert_eq!(report.conflict_count, 2);

    let conf = report
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::ConflictingDecision)
        .expect("conflicting decision expected");
    assert!(conf.shared_terms.contains(&"telemetry".to_string()));

    let ver = report
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::VersionDivergence)
        .expect("version divergence expected");
    assert!(ver.title.contains("react"));
    assert!(ver.summary.contains("^17.0.2") && ver.summary.contains("^19.0.0"));

    assert!(
        report
            .edges
            .iter()
            .any(|e| e.kind == DependencyKind::SharedStack),
        "expected SharedStack edge for common react + zustand packages"
    );
}

#[test]
fn maps_cross_functional_dependencies_via_entities_actions_and_mentions() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(
        &store,
        "col-apollo",
        "ApolloEngine",
        "v-main",
        &tmp.path().join("ApolloEngine"),
    );
    seed_collection(
        &store,
        "col-beacon",
        "BeaconPortal",
        "v-main",
        &tmp.path().join("BeaconPortal"),
    );

    let (fa, _) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "ApolloEngine\\plan.md",
        "ApolloEngine core storage integration.",
        now - 2 * DAY_MS,
        None,
    );
    let (fb, _) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "BeaconPortal\\roadmap.md",
        "BeaconPortal UI integrates the ApolloEngine query endpoint.",
        now - DAY_MS,
        None,
    );

    seed_entity(
        &store,
        "per-alice",
        "person",
        "Alice Mokoena",
        "{}",
        &[&fa, &fb],
    );
    seed_entity(
        &store,
        "dec-shared",
        "decision",
        "Use encrypted SQLCipher vault schema",
        "{}",
        &[&fa, &fb],
    );
    seed_entity(
        &store,
        "act-cross",
        "action_item",
        "Deliver schema migration handoff to BeaconPortal",
        r#"{"status":"open","project_hint":"BeaconPortal"}"#,
        &[&fa],
    );

    let report =
        scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
            .unwrap();

    assert!(report.edges.iter().any(|e| e.kind == DependencyKind::SharedStakeholder));
    assert!(report.edges.iter().any(|e| e.kind == DependencyKind::SharedDecision));
    assert!(report.edges.iter().any(|e| {
        e.kind == DependencyKind::CrossProjectAction
            && e.directed
            && e.from_project_id == "col-apollo"
            && e.to_project_id == "col-beacon"
    }));
    assert!(report.edges.iter().any(|e| {
        e.kind == DependencyKind::DocumentReference
            && e.directed
            && e.from_project_id == "col-beacon"
            && e.to_project_id == "col-apollo"
    }));

    let brief =
        prepare_cross_project_brief(store.conn(), Some("col-apollo"), Some("col-beacon"), false, false)
            .unwrap();
    assert!(!brief.citations.is_empty());
    assert!(brief.title.contains("ApolloEngine"));
}

#[test]
fn persists_insight_triage_status_and_filtering() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(&store, "c1", "AlphaOne", "v-main", &tmp.path().join("AlphaOne"));
    seed_collection(&store, "c2", "BetaTwo", "v-main", &tmp.path().join("BetaTwo"));

    seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "AlphaOne\\package.json",
        r#"{"dependencies": {"typescript": "^4.9.0"}}"#,
        now - 2 * DAY_MS,
        None,
    );
    seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "BetaTwo\\package.json",
        r#"{"dependencies": {"typescript": "^5.6.0"}}"#,
        now - DAY_MS,
        None,
    );

    let r1 = scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
        .unwrap();
    assert_eq!(r1.findings.len(), 1);
    let key = r1.findings[0].insight_key.clone();
    assert_eq!(r1.findings[0].status, InsightStatus::Open);

    set_insight_status(
        store.conn(),
        &key,
        InsightStatus::Acknowledged,
        Some("Planned upgrade in Q1"),
    )
    .unwrap();
    let r2 = scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
        .unwrap();
    assert_eq!(r2.findings[0].status, InsightStatus::Acknowledged);
    assert_eq!(r2.findings[0].note, "Planned upgrade in Q1");

    set_insight_status(store.conn(), &key, InsightStatus::Dismissed, None).unwrap();
    let r3 = scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
        .unwrap();
    assert!(r3.findings.is_empty());
    assert_eq!(r3.dismissed_count, 1);

    let r4 = scan_cross_project_intelligence(
        store.conn(),
        &CrossProjectOptions {
            include_dismissed: Some(true),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(r4.findings.len(), 1);
    assert_eq!(r4.findings[0].status, InsightStatus::Dismissed);
}

#[test]
fn excludes_backup_drives_and_quarantined_files_k2_k4() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("Main");
    let backup_root = tmp.path().join("Backup");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&backup_root).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", &main_root, Some("general"));
    seed_volume(&store, "v-backup", &backup_root, Some("backup"));

    seed_collection(&store, "c-live", "LiveAlpha", "v-main", &main_root.join("LiveAlpha"));
    seed_collection(
        &store,
        "c-mirror",
        "BackupMirror",
        "v-backup",
        &backup_root.join("BackupMirror"),
    );

    seed_doc(
        &store,
        "v-main",
        &main_root,
        "LiveAlpha\\package.json",
        r#"{"dependencies": {"vite": "^5.0.0"}}"#,
        now - DAY_MS,
        None,
    );
    let (fq, _) = seed_doc(
        &store,
        "v-main",
        &main_root,
        "LiveAlpha\\quarantined.md",
        "LiveAlpha confidential BackupMirror reference quarantined.",
        now - 2 * DAY_MS,
        None,
    );
    let (fd, _) = seed_doc(
        &store,
        "v-main",
        &main_root,
        "LiveAlpha\\deleted.md",
        "LiveAlpha BackupMirror reference deleted.",
        now - 3 * DAY_MS,
        None,
    );
    seed_doc(
        &store,
        "v-backup",
        &backup_root,
        "BackupMirror\\package.json",
        r#"{"dependencies": {"vite": "^3.0.0"}}"#,
        now - 4 * DAY_MS,
        None,
    );

    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![fq])
        .unwrap();
    store
        .with_full_sync_tx(|tx| soft_delete_file_in_tx(tx, &fd))
        .unwrap();

    let report =
        scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
            .unwrap();
    assert!(report.findings.is_empty());
    let live_summary = report
        .projects
        .iter()
        .find(|p| p.collection_id == "c-live")
        .unwrap();
    assert_eq!(live_summary.file_count, 1);
    let backup_summary = report
        .projects
        .iter()
        .find(|p| p.collection_id == "c-mirror")
        .unwrap();
    assert_eq!(backup_summary.file_count, 0);
}

#[test]
fn enforces_r7_privacy_gate_for_sensitive_project_files() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(&store, "c-pub", "PublicProj", "v-main", &tmp.path().join("PublicProj"));
    seed_collection(&store, "c-sec", "BoardProj", "v-main", &tmp.path().join("BoardProj"));

    let (f_pub, c_pub) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "PublicProj\\spec.md",
        "Hardware attestation design spec.",
        now - 5 * DAY_MS,
        Some([50u8; 32]),
    );
    let (f_sec, c_sec) = seed_doc(
        &store,
        "v-main",
        tmp.path(),
        "BoardProj\\executive_memo.md",
        "Confidential board compensation terms and acquisition PublicProj evaluation.",
        now - DAY_MS,
        Some([50u8; 32]),
    );
    set_file_sensitivity(store.conn(), &f_sec, "confidential").unwrap();

    upsert_chunk_embeddings(
        store.conn(),
        "test-embed",
        3,
        &[
            (
                PendingChunk {
                    chunk_id: c_pub,
                    chunk_uuid: "cp".into(),
                    file_id: f_pub,
                    embed_text: "p".into(),
                    content_hash: [1u8; 32],
                },
                vec![1.0, 0.0, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: c_sec,
                    chunk_uuid: "cs".into(),
                    file_id: f_sec.clone(),
                    embed_text: "s".into(),
                    content_hash: [2u8; 32],
                },
                vec![0.99, 0.14, 0.0],
            ),
        ],
    )
    .unwrap();

    let locked =
        scan_cross_project_intelligence(store.conn(), &CrossProjectOptions::default(), false)
            .unwrap();
    assert!(locked.findings.is_empty());
    assert!(locked.edges.is_empty());
    assert_eq!(locked.excluded_sensitive_count, 1);

    let err = scan_cross_project_intelligence(
        store.conn(),
        &CrossProjectOptions {
            include_sensitive: Some(true),
            ..Default::default()
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(err, AegisError::PermissionDenied(_)));

    let unlocked = scan_cross_project_intelligence(
        store.conn(),
        &CrossProjectOptions {
            include_sensitive: Some(true),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(unlocked.excluded_sensitive_count, 0);
    assert_eq!(unlocked.duplicate_effort_count, 1);
    assert!(unlocked.edges.iter().any(|e| e.kind == DependencyKind::DocumentReference));
}
