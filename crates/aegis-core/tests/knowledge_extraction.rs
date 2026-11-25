use std::fs;
use std::path::Path;

use aegis_core::db::SqliteStore;
use aegis_core::db::files::{
    DiscoveredFileInput, SENSITIVE_MASK, set_file_sensitivity, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::knowledge::{
    EntityType, KnowledgeExtractOptions, KnowledgeQueryOptions, ProposalEditInput, ProposalStatus,
    approve_proposal, audio_observation_enabled, build_knowledge_llm_prompt, dismiss_proposal,
    extract_knowledge_from_catalog, ingest_llm_candidates, load_file_text_for_llm,
    parse_llm_extraction_output, query_knowledge_dashboard, set_audio_observation_enabled,
    undo_approved_proposal, update_entity_status,
};
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

fn seed_file(
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
             VALUES (?1, ?2, ?3, ?4, 'home', 2, 1024, ?5, 0, 'hash', 0, 'fp-col')",
            params![format!("loc-{col_id}"), col_id, vol_id, root.to_string_lossy().to_string(), now],
        )
        .unwrap();
}

#[test]
fn deterministic_extractor_parses_people_orgs_projects_decisions_actions_and_vtt_transcripts() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let proj_dir = tmp.path().join("Projects\\Helios");
    seed_collection(&store, "col-helios", "Helios", "v-main", &proj_dir);

    let vtt_content = br#"WEBVTT

1
00:00:01.000 --> 00:00:05.000
<v Alice Mokoena>Attendees: Alice Mokoena, Bob Smith from Acme Corp.</v>

2
00:00:06.000 --> 00:00:10.000
<v Bob Smith>DECISION: Standardize offline SQLCipher vault across Helios.</v>

3
00:00:11.000 --> 00:00:16.000
<v Alice Mokoena>- [ ] Alice Mokoena to deliver key rotation guide by 2026-12-10</v>
"#;
    seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Helios\\Meetings\\Kickoff.vtt",
        vtt_content,
        now - 3 * DAY_MS,
    );

    let rep = extract_knowledge_from_catalog(store.conn(), &KnowledgeExtractOptions::default(), false)
        .unwrap();
    assert_eq!(rep.files_scanned, 1);
    assert!(rep.proposals_created >= 5);

    // K1: Extraction creates ONLY pending proposals; `entities` must stay 0 until owner approves!
    let dash = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert_eq!(dash.total_entities, 0);
    assert!(dash.pending_count >= 5);
    assert!(dash.proposals.iter().any(|p| p.entity_type == EntityType::Person && p.name == "Alice Mokoena"));
    assert!(dash.proposals.iter().any(|p| p.entity_type == EntityType::Organization && p.name == "Acme Corp"));
    assert!(dash.proposals.iter().any(|p| p.entity_type == EntityType::Project && p.name == "Helios"));
    assert!(dash.proposals.iter().any(|p| p.entity_type == EntityType::Decision && p.summary.contains("SQLCipher")));
    let act = dash.proposals.iter().find(|p| p.entity_type == EntityType::ActionItem).unwrap();
    assert_eq!(act.assignee.as_deref(), Some("Alice Mokoena"));
    assert_eq!(act.due_date.as_deref(), Some("2026-12-10"));
}

#[test]
fn incremental_merge_updates_existing_entity_and_undo_reverts_cleanly_k1() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let f1 = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Note1.md",
        b"Attendees: Alice Mokoena\nOrganization: Acme Corp\nProject: Atlas\n- [ ] Alice Mokoena to finish schema by 2026-12-01\n",
        now - 4 * DAY_MS,
    );
    let f2 = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\Note2.md",
        b"Attendees: Alice Mokoena\nVendor: Acme Corp\nDECISION: Ship release candidate in Q4\n",
        now - 2 * DAY_MS,
    );

    extract_knowledge_from_catalog(store.conn(), &KnowledgeExtractOptions::default(), false).unwrap();
    let dash1 = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();

    // Approve Alice, Project Atlas, and Acme Corp from Note1, plus Action Item.
    let p1_alice = dash1
        .proposals
        .iter()
        .find(|p| p.entity_type == EntityType::Person && p.name == "Alice Mokoena" && p.source_file_id.as_deref() == Some(&f1))
        .unwrap()
        .id
        .clone();
    let p1_proj = dash1
        .proposals
        .iter()
        .find(|p| p.entity_type == EntityType::Project && p.name == "Atlas")
        .unwrap()
        .id
        .clone();
    let p1_org = dash1
        .proposals
        .iter()
        .find(|p| p.entity_type == EntityType::Organization && p.name == "Acme Corp" && p.source_file_id.as_deref() == Some(&f1))
        .unwrap()
        .id
        .clone();
    let p1_act = dash1
        .proposals
        .iter()
        .find(|p| p.entity_type == EntityType::ActionItem)
        .unwrap()
        .id
        .clone();

    let res_alice1 = approve_proposal(&mut store, &p1_alice, None, false).unwrap();
    assert!(res_alice1.created_new);
    approve_proposal(
        &mut store,
        &p1_proj,
        Some(ProposalEditInput {
            name: Some("Atlas".into()),
            ..Default::default()
        }),
        false,
    )
    .unwrap();
    let res_org1 = approve_proposal(&mut store, &p1_org, None, false).unwrap();
    assert!(res_org1.created_new);

    let res_act = approve_proposal(
        &mut store,
        &p1_act,
        Some(ProposalEditInput {
            project_hint: Some("Atlas".into()),
            ..Default::default()
        }),
        false,
    )
    .unwrap();
    // Should create provenance + `owns_action` (Alice) + `belongs_to_project` (Atlas) = 3 links!
    assert_eq!(res_act.relationships_added, 3);

    // Toggle action item status (`open` -> `done`).
    update_entity_status(store.conn(), &res_act.entity_id, "done").unwrap();

    // Approving Note2's `Acme Corp` MUST merge into existing `res_org1.entity_id`, not create a duplicate!
    let p2_org = dash1
        .proposals
        .iter()
        .find(|p| p.entity_type == EntityType::Organization && p.name == "Acme Corp" && p.source_file_id.as_deref() == Some(&f2))
        .unwrap()
        .id
        .clone();
    let res_org2 = approve_proposal(&mut store, &p2_org, None, false).unwrap();
    assert!(!res_org2.created_new);
    assert!(res_org2.merged_into_existing);
    assert_eq!(res_org2.entity_id, res_org1.entity_id);

    let dash_merged = query_knowledge_dashboard(
        store.conn(),
        &KnowledgeQueryOptions {
            entity_type: Some(EntityType::Organization),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(dash_merged.entities.len(), 1);
    assert_eq!(dash_merged.entities[0].provenance.len(), 2);

    // Undoing p2_org removes Note2's provenance edge while keeping Note1's entity intact!
    undo_approved_proposal(&mut store, &p2_org).unwrap();
    let dash_after_undo_merge = query_knowledge_dashboard(
        store.conn(),
        &KnowledgeQueryOptions {
            entity_type: Some(EntityType::Organization),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(dash_after_undo_merge.entities.len(), 1);
    assert_eq!(dash_after_undo_merge.entities[0].provenance.len(), 1);

    // Undoing newly created p1_org deletes the entity completely!
    undo_approved_proposal(&mut store, &p1_org).unwrap();
    let dash_after_undo_orig = query_knowledge_dashboard(
        store.conn(),
        &KnowledgeQueryOptions {
            entity_type: Some(EntityType::Organization),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(dash_after_undo_orig.entities.len(), 0);
}

#[test]
fn idempotent_rescan_respects_watermarks_and_preserves_dismissed_decisions() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\Weekly.md",
        b"DECISION: Adopt deterministic parser\n",
        now - 2 * DAY_MS,
    );

    let rep1 = extract_knowledge_from_catalog(store.conn(), &KnowledgeExtractOptions::default(), false)
        .unwrap();
    assert_eq!(rep1.files_scanned, 1);
    assert_eq!(rep1.proposals_created, 1);

    let dash1 = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    dismiss_proposal(store.conn(), &dash1.proposals[0].id).unwrap();

    // Second scan without file edits hits watermark cleanly.
    let rep2 = extract_knowledge_from_catalog(store.conn(), &KnowledgeExtractOptions::default(), false)
        .unwrap();
    assert_eq!(rep2.files_scanned, 0);
    assert_eq!(rep2.files_skipped_unchanged, 1);

    // Forced rescan STILL preserves dismissed owner choice!
    let rep3 = extract_knowledge_from_catalog(
        store.conn(),
        &KnowledgeExtractOptions {
            force_rescan: Some(true),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(rep3.files_scanned, 1);
    assert_eq!(rep3.total_pending, 0);
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

    seed_volume(&store, "v-main", &main_root, Some("general"));
    seed_volume(&store, "v-bak", &bak_root, Some("backup"));

    let live_id = seed_file(
        &store,
        "v-main",
        &main_root,
        "Work\\Meeting.md",
        b"DECISION: Keep backup drives read-only\n",
        now - 3 * DAY_MS,
    );
    let quar_id = seed_file(
        &store,
        "v-main",
        &main_root,
        "Work\\Quar.md",
        b"DECISION: Should never extract quarantined files\n",
        now - 2 * DAY_MS,
    );
    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![quar_id])
        .unwrap();

    let bak_id = seed_file(
        &store,
        "v-bak",
        &bak_root,
        "Work\\BackupCopy.md",
        b"DECISION: Should never extract backup files\n",
        now - 2 * DAY_MS,
    );

    let rep = extract_knowledge_from_catalog(store.conn(), &KnowledgeExtractOptions::default(), false)
        .unwrap();
    assert_eq!(rep.files_scanned, 1);
    assert_eq!(rep.total_pending, 1);

    // K2/K4 refuse direct LLM text loads for quarantined and backup files.
    assert!(load_file_text_for_llm(store.conn(), &quar_id, false).is_err());
    assert!(load_file_text_for_llm(store.conn(), &bak_id, false).is_err());

    // Approve live proposal, then soft-delete the file -> provenance disappears cleanly (`K2`).
    let dash = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    approve_proposal(&mut store, &dash.proposals[0].id, None, false).unwrap();

    let dash_live = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert_eq!(dash_live.entities[0].provenance.len(), 1);

    store
        .with_full_sync_tx(|tx| soft_delete_file_in_tx(tx, &live_id))
        .unwrap();
    let dash_after_del = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert_eq!(dash_after_del.entities[0].provenance.len(), 0);
}

#[test]
fn audio_observation_is_off_by_default_and_pairs_companion_transcripts_when_opted_in_k4() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Meetings\\Weekly_Sync.m4a",
        b"fake-m4a-payload",
        now - 3 * DAY_MS,
    );
    seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Meetings\\Weekly_Sync.vtt",
        b"WEBVTT\n\n00:00:01.000 --> 00:00:03.000\n<v Alice Mokoena>DECISION: Pair local recordings only when opted in.</v>\n",
        now - 3 * DAY_MS,
    );

    assert!(!audio_observation_enabled(store.conn()).unwrap());
    let dash_off = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert!(!dash_off.audio_observation_enabled);
    assert!(dash_off.audio_recordings.is_empty());

    // Opt-in turns audio discovery ON and pairs `Weekly_Sync.m4a` -> `Weekly_Sync.vtt`.
    set_audio_observation_enabled(store.conn(), true).unwrap();
    let dash_on = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert!(dash_on.audio_observation_enabled);
    assert_eq!(dash_on.audio_recordings.len(), 1);
    assert_eq!(dash_on.audio_recordings[0].filename, "Weekly_Sync.m4a");
    assert_eq!(
        dash_on.audio_recordings[0].companion_transcript_name.as_deref(),
        Some("Weekly_Sync.vtt")
    );
}

#[test]
fn sensitive_files_skip_when_locked_and_mask_provenance_after_relock_r7() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let sec_id = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Vault\\Board_Compensation.md",
        b"DECISION: Approve executive bonus 99812 for Discovery",
        now - 2 * DAY_MS,
    );
    set_file_sensitivity(store.conn(), &sec_id, "confidential").unwrap();

    // Locked request explicitly asking for sensitive files MUST fail (`R7`).
    assert!(
        extract_knowledge_from_catalog(
            store.conn(),
            &KnowledgeExtractOptions {
                include_sensitive: Some(true),
                ..Default::default()
            },
            false,
        )
        .is_err()
    );
    assert!(load_file_text_for_llm(store.conn(), &sec_id, false).is_err());

    // Default locked scan skips sensitive files cleanly and counts them.
    let rep_locked =
        extract_knowledge_from_catalog(store.conn(), &KnowledgeExtractOptions::default(), false)
            .unwrap();
    assert_eq!(rep_locked.files_scanned, 0);
    assert_eq!(rep_locked.files_skipped_sensitive, 1);

    // Unlocked session extracts + approves sensitive decision cleanly.
    let rep_unlocked = extract_knowledge_from_catalog(
        store.conn(),
        &KnowledgeExtractOptions {
            include_sensitive: Some(true),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(rep_unlocked.files_scanned, 1);
    assert!(rep_unlocked.proposals_created >= 1);

    let dash_unlocked =
        query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), true).unwrap();
    let dec_id = dash_unlocked
        .proposals
        .iter()
        .find(|p| p.entity_type == EntityType::Decision)
        .unwrap()
        .id
        .clone();
    assert!(approve_proposal(&mut store, &dec_id, None, true).is_ok());

    // After relocking (`reveal_sensitive = false`), summary & provenance quotes are masked (`R7`)!
    let dash_relocked =
        query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert_eq!(dash_relocked.entities.len(), 1);
    assert_eq!(dash_relocked.entities[0].summary, SENSITIVE_MASK);
    assert_eq!(dash_relocked.entities[0].provenance[0].quote, SENSITIVE_MASK);
    assert!(dash_relocked.entities[0].provenance[0].redacted);
}

#[test]
fn llm_json_and_bullet_output_parser_ingests_pending_proposals() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let fid = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Research\\Notes.txt",
        b"Unstructured notes discussed offline architecture.",
        now - DAY_MS,
    );

    let (_fname, text, owning, _tr) = load_file_text_for_llm(store.conn(), &fid, false).unwrap();
    let prompt = build_knowledge_llm_prompt("Notes.txt", &text, owning.as_deref()).unwrap();
    assert!(prompt.1.contains("Notes.txt"));

    let simulated_llm_output = r#"[
      {"type":"decision","name":"Keep embeddings encrypted in SQLCipher","summary":"Use sqlite-vec inside AES-256 vault","evidence_quote":"offline architecture"},
      {"type":"action_item","name":"Profile llama3.2 RAM ceiling","assignee":"Thabo Ndlovu","due_date":"2026-12-15","project_hint":"Cairn"}
    ]"#;
    let cands = parse_llm_extraction_output(simulated_llm_output, "document");
    let upserted = ingest_llm_candidates(store.conn(), &fid, &cands, false).unwrap();
    assert_eq!(upserted, 2);

    let dash = query_knowledge_dashboard(store.conn(), &KnowledgeQueryOptions::default(), false).unwrap();
    assert_eq!(dash.pending_count, 2);
    assert!(dash.proposals.iter().all(|p| p.extractor == "local_llm" && p.status == ProposalStatus::Pending));
}
