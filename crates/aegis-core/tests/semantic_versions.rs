use std::fs;
use std::path::Path;

use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, rollback_batch, simulate_batch};
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, set_file_sensitivity, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::db::vec::{PendingChunk, upsert_chunk_embeddings};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use aegis_core::versions::{
    DiffLineKind, VersionDiffSummary, VersionLinkReason, VersionScanOptions,
    build_diff_llm_prompt, diff_version_pair, dismiss_version_chain, load_cached_diff_summary,
    pin_latest_version, propose_version_archive, propose_version_quarantine_dupes,
    save_cached_diff_summary, scan_version_chains, undismiss_version_chain,
};
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

fn seed_file_with_mtime(
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
        .execute(
            "UPDATE files SET mtime = ?1, size = ?2 WHERE id = ?3",
            params![mtime_ms, bytes.len() as i64, fid],
        )
        .unwrap();
    fid
}

fn seed_collection_with_location(
    store: &SqliteStore,
    col_id: &str,
    name: &str,
    vol_id: &str,
    root_path: &Path,
) {
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
             VALUES (?1, ?2, ?3, ?4, 'home', 3, 1024, ?5, 0, 'hash', 0, 'fp-atlas')",
            params![format!("loc-{col_id}"), col_id, vol_id, root_path.to_string_lossy().to_string(), now],
        )
        .unwrap();
}

#[test]
fn filename_and_versions_folder_chains_order_and_pick_latest() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    let proj_dir = tmp.path().join("Projects\\Atlas");
    seed_collection_with_location(&store, "col-atlas", "Atlas", "v-main", &proj_dir);

    let v1_id = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Atlas\\Versions\\2026-03-10 Desktop\\Budget_v1.md",
        b"# Budget v1\nOld plan",
        now - 20 * DAY_MS,
    );
    let v2_id = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Atlas\\Budget_v2.md",
        b"# Budget v2\nExpanded scope",
        now - 10 * DAY_MS,
    );
    let v2_dup_id = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Atlas\\Budget_v2 (1).md",
        b"# Budget v2\nExpanded scope",
        now - 8 * DAY_MS,
    );
    let final_id = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Atlas\\Budget_FINAL.md",
        b"# Budget Final\nSigned off scope",
        now - 2 * DAY_MS,
    );

    // Non-version episode sequence should NOT form a version chain.
    seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\Lesson_01.md",
        b"Lesson 1 text",
        now - 5 * DAY_MS,
    );
    seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\Lesson_02.md",
        b"Lesson 2 text",
        now - 4 * DAY_MS,
    );

    let report = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(report.total_chains, 1);
    let ch = &report.chains[0];
    assert_eq!(ch.canonical_stem, "Budget");
    assert_eq!(ch.collection_name.as_deref(), Some("Atlas"));
    assert!(ch.has_versions_folder);
    assert_eq!(ch.latest_file_id, final_id);
    assert_eq!(ch.exact_duplicate_count, 1);
    assert_eq!(ch.nodes.len(), 4);
    assert_eq!(ch.nodes[0].file_id, v1_id);
    assert!(ch.nodes[0].in_versions_folder);
    assert_eq!(ch.nodes[1].file_id, v2_id);
    assert!(ch.nodes[1].is_exact_duplicate);
    assert_eq!(ch.nodes[1].duplicate_of_file_id.as_deref(), Some(v2_dup_id.as_str()));
    assert!(!ch.nodes[2].is_exact_duplicate);
    assert_eq!(ch.nodes[3].file_id, final_id);
    assert!(ch.nodes[3].is_latest);
    assert!(ch.link_reasons.contains(&VersionLinkReason::FilenamePattern));
    assert!(ch.link_reasons.contains(&VersionLinkReason::VersionsFolder));
}

#[test]
fn file_history_links_renamed_versions_and_records_former_paths() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    // Moved earlier via file_history from C:\Inbox\Charter_draft.md -> C:\Archive\Executive_Charter.md,
    // while C:\Inbox\Charter_FINAL.md still sits in Inbox.
    let moved_id = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Archive\\Executive_Charter.md",
        b"# Executive Charter\nInitial draft moved to archive",
        now - 15 * DAY_MS,
    );
    let current_id = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Inbox\\Charter_FINAL.md",
        b"# Executive Charter\nRevised signed charter",
        now - 3 * DAY_MS,
    );

    let old_path = tmp.path().join("Inbox\\Charter_draft.md");
    let moved_path: String = store
        .conn()
        .query_row("SELECT path FROM files WHERE id = ?1", params![moved_id], |r| r.get(0))
        .unwrap();

    store
        .conn()
        .execute(
            "INSERT INTO action_batches (id, origin, status, total_actions, created_at)
             VALUES ('b-hist', 'filing:test', 'executed', 1, ?1)",
            params![now - 10 * DAY_MS],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO file_history (id, file_id, old_path, new_path, batch_id, changed_at)
             VALUES ('fh-1', ?1, ?2, ?3, 'b-hist', ?4)",
            params![moved_id, old_path.to_string_lossy().to_string(), moved_path, now - 10 * DAY_MS],
        )
        .unwrap();

    let report = scan_version_chains(
        store.conn(),
        &VersionScanOptions {
            signal: Some(VersionLinkReason::FileHistory),
            ..Default::default()
        },
        now,
        false,
    )
    .unwrap();
    assert_eq!(report.total_chains, 1);
    let ch = &report.chains[0];
    assert!(ch.has_file_history);
    assert_eq!(ch.version_count, 2);
    assert_eq!(ch.latest_file_id, current_id);
    assert_eq!(ch.nodes[0].file_id, moved_id);
    assert_eq!(
        ch.nodes[0].former_paths,
        vec![old_path.to_string_lossy().to_string()]
    );
    assert!(ch.nodes[0].link_reasons.contains(&VersionLinkReason::FileHistory));
    assert!(ch.nodes[1].link_reasons.contains(&VersionLinkReason::FileHistory));
}

#[test]
fn semantic_vector_similarity_links_renamed_drafts_and_respects_r7() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    // Two completely differently named drafts in the same folder with cosine >= 0.92.
    let f1 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Initial_Notes.md",
        b"Local zero-network RAG engine and biometric vault.",
        now - 12 * DAY_MS,
    );
    let f2 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Board_Whitepaper.md",
        b"Local zero-network RAG engine with hardware biometric keys and SQLCipher.",
        now - 4 * DAY_MS,
    );
    // Locked confidential file with matching filename stem & identical vector must stay isolated when locked!
    let sec_f3 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Secret_Memo.md",
        b"Top secret executive compensation terms 99812.",
        now - 2 * DAY_MS,
    );

    insert_chunk(
        store.conn(),
        &f1,
        0,
        "Local zero-network RAG engine and biometric vault.",
        8,
        true,
    )
    .unwrap();
    insert_chunk(
        store.conn(),
        &f2,
        0,
        "Local zero-network RAG engine with hardware biometric keys and SQLCipher.",
        10,
        true,
    )
    .unwrap();
    insert_chunk(
        store.conn(),
        &sec_f3,
        0,
        "Top secret executive compensation terms 99812.",
        7,
        true,
    )
    .unwrap();
    set_file_sensitivity(store.conn(), &sec_f3, "confidential").unwrap();

    let c1: i64 = store
        .conn()
        .query_row("SELECT id FROM chunks WHERE file_id = ?1", params![f1], |r| r.get(0))
        .unwrap();
    let c2: i64 = store
        .conn()
        .query_row("SELECT id FROM chunks WHERE file_id = ?1", params![f2], |r| r.get(0))
        .unwrap();
    let c3: i64 = store
        .conn()
        .query_row("SELECT id FROM chunks WHERE file_id = ?1", params![sec_f3], |r| r.get(0))
        .unwrap();

    upsert_chunk_embeddings(
        store.conn(),
        "test-embed",
        3,
        &[
            (
                PendingChunk {
                    chunk_id: c1,
                    chunk_uuid: "cu1".into(),
                    file_id: f1.clone(),
                    embed_text: "d1".into(),
                    content_hash: [1u8; 32],
                },
                vec![1.0, 0.0, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: c2,
                    chunk_uuid: "cu2".into(),
                    file_id: f2.clone(),
                    embed_text: "d2".into(),
                    content_hash: [2u8; 32],
                },
                vec![0.97, 0.243, 0.0], // cosine ~ 0.97
            ),
            (
                PendingChunk {
                    chunk_id: c3,
                    chunk_uuid: "cu3".into(),
                    file_id: sec_f3.clone(),
                    embed_text: "d3".into(),
                    content_hash: [3u8; 32],
                },
                vec![0.99, 0.141, 0.0], // cosine ~ 0.99 with f1, but sensitive!
            ),
        ],
    )
    .unwrap();

    // Locked mode: only f1 + f2 link semantically; sec_f3 is excluded from semantic similarity!
    let locked = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(locked.total_chains, 1);
    assert_eq!(locked.semantic_chains_count, 1);
    let ch_locked = &locked.chains[0];
    assert_eq!(ch_locked.version_count, 2);
    assert!(ch_locked.nodes.iter().all(|n| n.file_id != sec_f3));
    assert!(ch_locked.nodes[1].semantic_similarity_prev.is_some_and(|s| s >= 0.95));

    // Locked diff on sec_f3 is strictly redacted (R7).
    let locked_diff = diff_version_pair(store.conn(), &f1, &sec_f3, false).unwrap();
    assert!(locked_diff.redacted_sensitive);
    assert_eq!(locked_diff.diff_kind, "redacted_sensitive");
    assert!(locked_diff.hunks.is_empty());
    assert!(locked_diff.semantic_similarity.is_none());
    assert!(build_diff_llm_prompt(&locked_diff).is_none());

    // Attempting to save summary for sensitive pair is silently refused (R7).
    save_cached_diff_summary(
        store.conn(),
        &VersionDiffSummary {
            older_file_id: f1.clone(),
            newer_file_id: sec_f3.clone(),
            summary: "Should never cache".into(),
            badges: vec![],
            model_id: "m1".into(),
            model_name: "m1".into(),
            backend: "gguf".into(),
            cached: false,
            duration_ms: 1,
        },
    )
    .unwrap();
    assert!(
        load_cached_diff_summary(store.conn(), &f1, &sec_f3, "m1", "m1", "gguf")
            .unwrap()
            .is_none()
    );

    // Unlocked mode: sec_f3 joins via semantic vector similarity AND its diff reveals hunks.
    let unlocked = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, true).unwrap();
    assert_eq!(unlocked.total_chains, 1);
    assert_eq!(unlocked.chains[0].version_count, 3);
    assert!(unlocked.chains[0].has_sensitive);

    let unlocked_diff = diff_version_pair(store.conn(), &f1, &sec_f3, true).unwrap();
    assert!(!unlocked_diff.redacted_sensitive);
    assert_eq!(unlocked_diff.diff_kind, "text");
    assert!(!unlocked_diff.hunks.is_empty());
}

#[test]
fn excludes_backup_drives_and_quarantined_files_k2_k4() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    let main_root = tmp.path().join("main");
    let bak_root = tmp.path().join("backup");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&bak_root).unwrap();

    seed_volume(&store, "v-main", &main_root, "online", Some("general"));
    seed_volume(&store, "v-bak", &bak_root, "online", Some("backup"));

    let _live = seed_file_with_mtime(
        &store,
        "v-main",
        &main_root,
        "Work\\Spec_v1.md",
        b"live spec",
        now - 10 * DAY_MS,
    );
    let quar_id = seed_file_with_mtime(
        &store,
        "v-main",
        &main_root,
        "Work\\Spec_v2.md",
        b"quar spec",
        now - 5 * DAY_MS,
    );
    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![quar_id])
        .unwrap();

    let deleted_id = seed_file_with_mtime(
        &store,
        "v-main",
        &main_root,
        "Work\\Spec_v3.md",
        b"deleted spec",
        now - 3 * DAY_MS,
    );
    store
        .with_full_sync_tx(|tx| soft_delete_file_in_tx(tx, &deleted_id))
        .unwrap();

    let bak1 = seed_file_with_mtime(
        &store,
        "v-bak",
        &bak_root,
        "Backup\\Report_v1.md",
        b"bak 1",
        now - 10 * DAY_MS,
    );
    let bak2 = seed_file_with_mtime(
        &store,
        "v-bak",
        &bak_root,
        "Backup\\Report_v2.md",
        b"bak 2",
        now - 5 * DAY_MS,
    );

    let report = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(report.total_chains, 0);

    // K4 refuses direct diff on backup files; K2 refuses diff on quarantined files.
    assert!(diff_version_pair(store.conn(), &bak1, &bak2, false).is_err());
    assert!(diff_version_pair(store.conn(), &_live, &quar_id, false).is_err());
}

#[test]
fn deterministic_text_diff_detects_sections_terms_and_hunks() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    let older = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Governance_v1.md",
        b"# Overview\nUse legacy cloud backup sync.\nStore tokens cleanly.\n",
        now - 10 * DAY_MS,
    );
    let newer = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Governance_v2.md",
        b"# Overview\n## Biometric Access Control\nUse hardware enclave credentials.\nStore encrypted SQLCipher metadata locally.\nStore tokens cleanly.\n",
        now - 2 * DAY_MS,
    );

    let diff = diff_version_pair(store.conn(), &older, &newer, false).unwrap();
    assert_eq!(diff.diff_kind, "text");
    assert!(!diff.identical_bytes);
    assert!(!diff.redacted_sensitive);
    assert_eq!(diff.removed_lines, 1);
    assert_eq!(diff.added_lines, 3);
    assert!(diff
        .conceptual_badges
        .iter()
        .any(|b| b.contains("Biometric Access Control")));
    assert!(diff.deterministic_summary.contains("Biometric Access Control"));
    assert!(diff.hunks[0].lines.iter().any(|l| l.kind == DiffLineKind::Added && l.text.contains("SQLCipher")));
    assert!(diff.hunks[0].lines.iter().any(|l| l.kind == DiffLineKind::Removed && l.text.contains("cloud")));

    let prompt = build_diff_llm_prompt(&diff).unwrap();
    assert!(prompt.1.contains("Governance_v2.md"));

    // Binary metadata diff fallback when comparing two unchunked binary formats.
    let img_v1 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Assets\\Poster_v1.psd",
        &vec![1u8; 4_096],
        now - 10 * DAY_MS,
    );
    let img_v2 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Assets\\Poster_v2.psd",
        &vec![2u8; 10_240],
        now - 2 * DAY_MS,
    );
    let bin_diff = diff_version_pair(store.conn(), &img_v1, &img_v2, false).unwrap();
    assert_eq!(bin_diff.diff_kind, "binary_metadata");
    assert!(bin_diff.hunks.is_empty());
    assert!(bin_diff.deterministic_summary.contains("grew"));
}

#[test]
fn propose_archive_and_quarantine_dupes_full_k1_lifecycle_with_rollback() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    let root = tmp.path().join("work");
    fs::create_dir_all(&root).unwrap();
    seed_volume(&store, "v-main", &root, "online", None);
    store.add_root(&root).unwrap();

    let v1_id = seed_file_with_mtime(
        &store,
        "v-main",
        &root,
        "Alpha\\Plan_v1.md",
        b"First revision unique words",
        now - 15 * DAY_MS,
    );
    let v2_a_id = seed_file_with_mtime(
        &store,
        "v-main",
        &root,
        "Alpha\\Plan_v2.md",
        b"Second revision identical content",
        now - 8 * DAY_MS,
    );
    let v2_b_id = seed_file_with_mtime(
        &store,
        "v-main",
        &root,
        "Alpha\\Plan_v2 (1).md",
        b"Second revision identical content",
        now - 7 * DAY_MS,
    );
    let v2_c_id = seed_file_with_mtime(
        &store,
        "v-main",
        &root,
        "Alpha\\Plan_v2 (2).md",
        b"Second revision identical content",
        now - 6 * DAY_MS,
    );
    let v3_latest_id = seed_file_with_mtime(
        &store,
        "v-main",
        &root,
        "Alpha\\Plan_FINAL.md",
        b"Final signed revision",
        now - DAY_MS,
    );

    // 1. FR-004.2: Quarantining non-identical semantic revisions MUST be refused.
    assert!(
        propose_version_quarantine_dupes(&mut store, &v3_latest_id, std::slice::from_ref(&v1_id)).is_err()
    );

    // 2. Archive v1 -> Alpha\Versions (propose -> simulate -> approve -> execute -> rollback).
    let versions_dir = root.join("Alpha\\Versions");
    let archive_batch = propose_version_archive(
        &mut store,
        std::slice::from_ref(&v1_id),
        &versions_dir.to_string_lossy(),
    )
    .unwrap();
    let sim = simulate_batch(&store, &archive_batch).unwrap();
    assert_eq!(sim.blocked_count, 0);
    assert_eq!(sim.ready_count, 1);

    approve_batch(&mut store, &archive_batch, None).unwrap();
    let exec_res = execute_batch(&mut store, &archive_batch).unwrap();
    assert_eq!(exec_res.executed_count, 1);
    assert!(versions_dir.join("Plan_v1.md").is_file());
    assert!(!root.join("Alpha\\Plan_v1.md").exists());

    // Rollback restores original file location cleanly.
    let rb = rollback_batch(&mut store, &archive_batch).unwrap();
    assert_eq!(rb.reverted_count, 1);
    assert!(root.join("Alpha\\Plan_v1.md").is_file());

    // 3. Quarantine 1 exact duplicate when 3 copies exist (keeps MIN_COPIES = 2), then reject dropping to 1 copy!
    let q_batch = propose_version_quarantine_dupes(
        &mut store,
        &v2_c_id,
        std::slice::from_ref(&v2_a_id),
    )
    .unwrap();
    let q_sim = simulate_batch(&store, &q_batch).unwrap();
    assert_eq!(q_sim.blocked_count, 0);

    approve_batch(&mut store, &q_batch, None).unwrap();
    let q_exec = execute_batch(&mut store, &q_batch).unwrap();
    assert_eq!(q_exec.executed_count, 1);
    assert!(!root.join("Alpha\\Plan_v2.md").exists());

    // Trying to quarantine v2_b when only 2 copies remain MUST block (min_copies = 2)!
    assert!(
        propose_version_quarantine_dupes(&mut store, &v2_c_id, std::slice::from_ref(&v2_b_id)).is_err()
    );

    // Rollback quarantine restores v2_a cleanly!
    let q_rb = rollback_batch(&mut store, &q_batch).unwrap();
    assert_eq!(q_rb.reverted_count, 1);
    assert!(root.join("Alpha\\Plan_v2.md").is_file());
}

#[test]
fn pin_latest_and_dismiss_persist_across_scans() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), "online", None);

    let f1 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\Pitch_v1.md",
        b"Pitch 1",
        now - 10 * DAY_MS,
    );
    let _f2 = seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\Pitch_v2.md",
        b"Pitch 2",
        now - 2 * DAY_MS,
    );

    let rep1 = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(rep1.total_chains, 1);
    let ck = rep1.chains[0].chain_key.clone();
    assert_ne!(rep1.chains[0].latest_file_id, f1);

    pin_latest_version(store.conn(), &ck, &f1).unwrap();
    let rep2 = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(rep2.chains[0].latest_file_id, f1);
    assert_eq!(rep2.chains[0].pinned_latest_file_id.as_deref(), Some(f1.as_str()));

    dismiss_version_chain(store.conn(), &ck).unwrap();
    let hidden = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(hidden.total_chains, 0);
    assert_eq!(hidden.dismissed_count, 1);

    let shown = scan_version_chains(
        store.conn(),
        &VersionScanOptions {
            include_dismissed: Some(true),
            ..Default::default()
        },
        now,
        false,
    )
    .unwrap();
    assert_eq!(shown.total_chains, 1);
    assert!(shown.chains[0].dismissed);

    undismiss_version_chain(store.conn(), &ck).unwrap();
    let restored = scan_version_chains(store.conn(), &VersionScanOptions::default(), now, false).unwrap();
    assert_eq!(restored.total_chains, 1);
    assert!(!restored.chains[0].dismissed);
    // Pinned choice stays intact across dismiss -> undismiss!
    assert_eq!(restored.chains[0].latest_file_id, f1);
}
