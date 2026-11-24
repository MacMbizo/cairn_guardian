use std::fs;
use std::path::Path;

use aegis_core::contracts::{
    ActionStatus, ActionType, BatchStatus, ConflictResolutionChoice, RollbackConflictKind,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{
    approve_batch, execute_batch, propose_batch, resolve_rollback_conflict, rollback_batch,
    simulate_batch,
};
use aegis_core::db::files::{DiscoveredFileInput, set_file_sensitivity, upsert_discovered_file};
use aegis_core::doctor::run_doctor;
use aegis_core::error::AegisError;
use aegis_core::folderless::{FolderlessRuleInput, list_folderless_rules, save_folderless_rule};
use aegis_core::healing::{
    HealResolutionSource, HealResolutionStatus, HealScanOptions, TagWeedKind, dismiss_heal_issue,
    heal_folderless_rule_path, list_heal_batches, preview_heal_patches, propose_heal_batch,
    scan_workspace_healing, undismiss_heal_issue,
};
use aegis_core::contracts::{ProposedActionInput, RefilePlan, TargetKind};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use rusqlite::params;
use tempfile::TempDir;

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
    text: &str,
) -> String {
    let file_path = root.join(rel_path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let bytes = text.as_bytes();
    fs::write(&file_path, bytes).unwrap();
    let st = inspect_path_metadata(&file_path).unwrap().unwrap();
    let now = aegis_core::now_ms();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: None,
        path: file_path,
        size: bytes.len() as i64,
        mtime: now,
        ctime: now,
        blake3: Some(blake3::hash(bytes).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    upsert_discovered_file(store.conn(), &input).unwrap()
}

fn record_scanner_rename(
    store: &SqliteStore,
    vol_id: &str,
    old_abs: &Path,
    new_abs: &Path,
) -> String {
    if let Some(parent) = new_abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::rename(old_abs, new_abs).unwrap();
    let bytes = fs::read(new_abs).unwrap();
    let st = inspect_path_metadata(new_abs).unwrap().unwrap();
    let now = aegis_core::now_ms();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: None,
        path: new_abs.to_path_buf(),
        size: bytes.len() as i64,
        mtime: now,
        ctime: now,
        blake3: Some(blake3::hash(&bytes).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    upsert_discovered_file(store.conn(), &input).unwrap()
}

#[test]
fn heals_markdown_and_wikilinks_after_usn_scanner_rename_with_k1_rollback() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    // Target 1: renamed externally in explorer -> picked up by upsert_discovered_file -> heal_move_log.
    let old_spec = tmp.path().join("Vault\\Drafts\\q4 spec.md");
    let new_spec = tmp.path().join("Vault\\Archives\\2026\\Q4 Final Spec.md");
    let _f_spec = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Vault\\Drafts\\q4 spec.md",
        "# Q4 Spec\nFinal deliverables.",
    );
    let _moved_spec_id = record_scanner_rename(&store, "v-main", &old_spec, &new_spec);

    // Target 2: moved earlier via file_history.
    let old_policy = tmp.path().join("Vault\\vendor-policy.md");
    let new_policy = tmp.path().join("Vault\\Policies\\Vendor Governance.md");
    let f_policy = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Vault\\Policies\\Vendor Governance.md",
        "# Vendor Governance\nAlways audit.",
    );
    store
        .conn()
        .execute(
            "INSERT INTO file_history (id, file_id, old_path, new_path, batch_id, changed_at)
             VALUES ('fh-pol', ?1, ?2, ?3, 'b-prev', ?4)",
            params![
                f_policy,
                old_policy.to_string_lossy().to_string(),
                new_policy.to_string_lossy().to_string(),
                aegis_core::now_ms(),
            ],
        )
        .unwrap();

    let orig_note = "\
# Weekly Review
See [Q4 Plan](Drafts/q4%20spec.md#Summary) for budget.
Also check [[vendor-policy|Vendor Rules]].
External [Web](https://example.com/Drafts/q4%20spec.md) stays untouched.
";
    let note_id = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Vault\\Weekly Review.md",
        orig_note,
    );
    let note_disk_path = tmp.path().join("Vault\\Weekly Review.md");

    let report = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert!(report.move_log_entries >= 1);
    assert_eq!(report.broken_links.len(), 2, "got: {:#?}", report.broken_links);

    let md_issue = report
        .broken_links
        .iter()
        .find(|l| l.broken_target == "Drafts/q4 spec.md")
        .unwrap();
    assert_eq!(md_issue.status, HealResolutionStatus::Resolved);
    assert_eq!(
        md_issue.resolution_source,
        Some(HealResolutionSource::HealMoveLog)
    );
    assert_eq!(
        md_issue.suggested_target.as_deref(),
        Some("Archives/2026/Q4%20Final%20Spec.md")
    );

    let wiki_issue = report
        .broken_links
        .iter()
        .find(|l| l.broken_target == "vendor-policy")
        .unwrap();
    assert_eq!(wiki_issue.status, HealResolutionStatus::Resolved);
    assert_eq!(
        wiki_issue.resolution_source,
        Some(HealResolutionSource::FileHistory)
    );
    assert_eq!(
        wiki_issue.suggested_target.as_deref(),
        Some("Vendor Governance")
    );

    let keys: Vec<String> = report
        .broken_links
        .iter()
        .map(|l| l.issue_key.clone())
        .collect();
    let previews = preview_heal_patches(&store, &keys, &[], false, false).unwrap();
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].links_healed, 2);
    assert!(previews[0].blocked_reason.is_none());
    // K1: Scan & preview must not alter disk.
    assert_eq!(fs::read_to_string(&note_disk_path).unwrap(), orig_note);

    let batch_id = propose_heal_batch(&mut store, &keys, &[], false, false).unwrap();
    let sim = simulate_batch(&store, &batch_id).unwrap();
    assert_eq!(sim.items.len(), 1);
    assert!(sim.items[0].policy_allowed);
    assert_eq!(fs::read_to_string(&note_disk_path).unwrap(), orig_note);

    approve_batch(&mut store, &batch_id, None).unwrap();
    let exec = execute_batch(&mut store, &batch_id).unwrap();
    assert_eq!(exec.status, BatchStatus::Executed);
    assert_eq!(exec.executed_count, 1);
    assert_eq!(exec.actions[0].action_type, ActionType::PatchText);
    assert_eq!(exec.actions[0].status, ActionStatus::Executed);

    let healed_disk = fs::read_to_string(&note_disk_path).unwrap();
    assert!(healed_disk.contains("[Q4 Plan](Archives/2026/Q4%20Final%20Spec.md#Summary)"));
    assert!(healed_disk.contains("[[Vendor Governance|Vendor Rules]]"));
    assert!(healed_disk.contains("https://example.com/Drafts/q4%20spec.md"));

    // Verify FTS5 & chunks were refreshed cleanly.
    let fts_hit: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE file_id = ?1 AND text LIKE '%Vendor Governance%'",
            params![note_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_hit, 1);

    // Recent batch summary shows undo availability.
    let batches = list_heal_batches(store.conn(), 10).unwrap();
    assert_eq!(batches.len(), 1);
    assert!(batches[0].can_undo);

    // Rescan confirms 0 broken links left!
    let rescan = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(rescan.broken_links.len(), 0);

    // K1 rollback restores original file bytes verbatim.
    let rb = rollback_batch(&mut store, &batch_id).unwrap();
    assert_eq!(rb.status, BatchStatus::RolledBack);
    assert_eq!(rb.reverted_count, 1);
    assert_eq!(fs::read_to_string(&note_disk_path).unwrap(), orig_note);
}

#[test]
fn detects_and_consolidates_tag_weeds_and_flags_hex_colors_without_mutating_code() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let n1_orig = "\
---
title: Standards
tags: [Architecture, \"#A3E2F1\", govrenance]
---
Tracking #governance and #architecture (#phase1, #2026).
Body accent color is #A3E2F1.
```css
.token { color: #A3E2F1; border: #f00bar; }
```
";
    let n2_orig = "\
Tracking #governance, #phase2, and #ARCHITECTURE across notes.
";

    let _f1 = seed_file(&store, "v-main", tmp.path(), "Notes\\n1.md", n1_orig);
    let _f2 = seed_file(&store, "v-main", tmp.path(), "Notes\\n2.md", n2_orig);

    let report = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert!(
        report
            .tag_clusters
            .iter()
            .any(|c| c.kind == TagWeedKind::CaseMismatch && c.canonical_tag == "architecture")
    );
    assert!(
        report
            .tag_clusters
            .iter()
            .any(|c| c.kind == TagWeedKind::TypoOrVariant && c.canonical_tag == "governance")
    );
    assert!(
        report
            .tag_clusters
            .iter()
            .any(|c| c.kind == TagWeedKind::HexColorFalsePositive)
    );
    // Series #phase1 vs #phase2 must NOT be misclassified as typo variants!
    assert!(
        !report
            .tag_clusters
            .iter()
            .any(|c| c.canonical_tag.contains("phase"))
    );

    let cluster_keys: Vec<String> = report
        .tag_clusters
        .iter()
        .filter(|c| c.patchable_count > 0)
        .map(|c| c.issue_key.clone())
        .collect();
    let batch_id = propose_heal_batch(&mut store, &[], &cluster_keys, false, false).unwrap();
    approve_batch(&mut store, &batch_id, None).unwrap();
    let exec = execute_batch(&mut store, &batch_id).unwrap();
    assert_eq!(exec.status, BatchStatus::Executed);

    let n1_after = fs::read_to_string(tmp.path().join("Notes\\n1.md")).unwrap();
    assert!(n1_after.contains("tags: [architecture, governance]"));
    // Body CSS block and prose hex color untouched:
    assert!(n1_after.contains(".token { color: #A3E2F1; border: #f00bar; }"));
    assert!(n1_after.contains("Body accent color is #A3E2F1."));

    let n2_after = fs::read_to_string(tmp.path().join("Notes\\n2.md")).unwrap();
    assert!(n2_after.contains("#architecture"));

    rollback_batch(&mut store, &batch_id).unwrap();
    assert_eq!(fs::read_to_string(tmp.path().join("Notes\\n1.md")).unwrap(), n1_orig);
    assert_eq!(fs::read_to_string(tmp.path().join("Notes\\n2.md")).unwrap(), n2_orig);
}

#[test]
fn toctou_blocks_patch_if_file_edited_after_propose_and_rollback_detects_post_heal_edits() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let old_p = tmp.path().join("old.md");
    let new_p = tmp.path().join("sub\\moved.md");
    let _t1 = seed_file(&store, "v-main", tmp.path(), "old.md", "Target");
    let _t2 = record_scanner_rename(&store, "v-main", &old_p, &new_p);

    let _src = seed_file(&store, "v-main", tmp.path(), "index.md", "See [Old](old.md).");
    let index_disk = tmp.path().join("index.md");

    let r1 = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    let keys: Vec<String> = r1.broken_links.iter().map(|l| l.issue_key.clone()).collect();
    let batch1 = propose_heal_batch(&mut store, &keys, &[], false, false).unwrap();
    approve_batch(&mut store, &batch1, None).unwrap();

    // Part A: external edit on disk BEFORE execute -> TOCTOU must abort without overwriting edit!
    let external_edit = "See [Old](old.md) — urgent external edit.";
    fs::write(&index_disk, external_edit).unwrap();
    let res1 = execute_batch(&mut store, &batch1).unwrap();
    assert_eq!(res1.failed_count, 1);
    assert_eq!(res1.status, BatchStatus::Failed);
    assert_eq!(fs::read_to_string(&index_disk).unwrap(), external_edit);

    // Part B: re-index edited file, heal cleanly, then simulate post-heal edit -> rollback conflict!
    let _src_updated = seed_file(&store, "v-main", tmp.path(), "index.md", external_edit);
    let r2 = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    let keys2: Vec<String> = r2.broken_links.iter().map(|l| l.issue_key.clone()).collect();
    let batch2 = propose_heal_batch(&mut store, &keys2, &[], false, false).unwrap();
    approve_batch(&mut store, &batch2, None).unwrap();
    let res2 = execute_batch(&mut store, &batch2).unwrap();
    assert_eq!(res2.status, BatchStatus::Executed);

    let healed = fs::read_to_string(&index_disk).unwrap();
    assert!(healed.contains("[Old](sub/moved.md)"));

    let post_heal_edit = format!("{healed}\nUser post-heal notes.");
    fs::write(&index_disk, &post_heal_edit).unwrap();

    let rb = rollback_batch(&mut store, &batch2).unwrap();
    assert_eq!(rb.status, BatchStatus::Partial);
    assert_eq!(rb.conflicts.len(), 1);
    assert_eq!(rb.conflicts[0].conflict, RollbackConflictKind::Modified);
    assert_eq!(
        rb.conflicts[0].recommended_choice,
        ConflictResolutionChoice::CopyBackKeepingBoth
    );

    let action_id = rb.conflicts[0].action_id.clone();
    let resolved = resolve_rollback_conflict(
        &mut store,
        &action_id,
        ConflictResolutionChoice::CopyBackKeepingBoth,
    )
    .unwrap();
    assert_eq!(resolved.status, BatchStatus::RolledBack);
    assert_eq!(fs::read_to_string(&index_disk).unwrap(), post_heal_edit);
    assert_eq!(
        fs::read_to_string(tmp.path().join("index (2).md")).unwrap(),
        external_edit
    );
}

#[test]
fn enforces_r7_privacy_gate_for_sensitive_markdown_notes() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let old_t = tmp.path().join("Docs\\tax.md");
    let new_t = tmp.path().join("Docs\\2026\\tax_final.md");
    let _t = seed_file(&store, "v-main", tmp.path(), "Docs\\tax.md", "Tax notes");
    let _moved = record_scanner_rename(&store, "v-main", &old_t, &new_t);

    let sec_id = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Private\\passport_notes.md",
        "Private ref [Tax](../Docs/tax.md).",
    );
    set_file_sensitivity(store.conn(), &sec_id, "confidential").unwrap();

    let locked = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(locked.broken_links.len(), 0);
    assert_eq!(locked.excluded_sensitive_count, 1);

    let denied = scan_workspace_healing(
        store.conn(),
        &HealScanOptions {
            include_sensitive: true,
            ..Default::default()
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(denied, AegisError::PermissionDenied(_)));

    let unlocked = scan_workspace_healing(
        store.conn(),
        &HealScanOptions {
            include_sensitive: true,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(unlocked.excluded_sensitive_count, 0);
    assert_eq!(unlocked.broken_links.len(), 1);
    assert!(unlocked.broken_links[0].sensitive);
}

#[test]
fn excludes_backup_drives_quarantined_files_and_flags_quarantined_link_targets_k2_k4() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("main");
    let backup_root = tmp.path().join("backup");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&backup_root).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", &main_root, None);
    seed_volume(&store, "v-bak", &backup_root, Some("backup"));
    store.add_root(&main_root).unwrap();

    // Backup drive note with broken link must NEVER be scanned or touched (K4).
    let _bak_note = seed_file(
        &store,
        "v-bak",
        &backup_root,
        "bak_note.md",
        "Broken [BackupTarget](missing.md).",
    );

    // Quarantined target on main volume via real K1 quarantine workflow.
    let q_target = seed_file(&store, "v-main", &main_root, "assets\\diagram.png", "pngdata");
    let q_path = main_root.join("assets\\diagram.png");
    let q_batch = propose_batch(
        &mut store,
        &RefilePlan {
            origin: "test".into(),
            actions: vec![ProposedActionInput {
                agent: Some("Housekeeper".into()),
                action_type: ActionType::Quarantine,
                target_kind: TargetKind::File,
                file_id: Some(q_target),
                atomic_unit_id: None,
                from_path: q_path.to_string_lossy().to_string(),
                to_path: None,
                reason: "Quarantine test".into(),
                confidence: 0.99,
                evidence_json: "{}".into(),
            }],
        },
    )
    .unwrap();
    approve_batch(&mut store, &q_batch, None).unwrap();
    execute_batch(&mut store, &q_batch).unwrap();

    // Active note linking to that quarantined file must surface InQuarantine, never auto-patch.
    let _note = seed_file(
        &store,
        "v-main",
        &main_root,
        "readme.md",
        "See ![Diagram](assets/diagram.png).",
    );

    let report = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(report.notes_scanned, 1);
    assert_eq!(report.broken_links.len(), 1);
    assert_eq!(
        report.broken_links[0].status,
        HealResolutionStatus::InQuarantine
    );
    assert!(report.broken_links[0].quarantine_entry_id.is_some());

    // Staging an InQuarantine link must refuse (nothing auto-patchable).
    let keys = vec![report.broken_links[0].issue_key.clone()];
    let err = propose_heal_batch(&mut store, &keys, &[], false, false).unwrap_err();
    assert!(matches!(err, AegisError::InvalidInput(_)));
}

#[test]
fn detects_stale_folderless_rule_paths_and_persists_dismissals() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let old_inbox = tmp.path().join("OldInbox");
    let new_inbox = tmp.path().join("NewInbox");
    let dest = tmp.path().join("Archives");
    fs::create_dir_all(&new_inbox).unwrap();
    fs::create_dir_all(&dest).unwrap();

    store
        .conn()
        .execute(
            "INSERT INTO heal_move_log (id, file_id, volume_id, old_path, new_path, source, detected_at)
             VALUES ('m-dir', NULL, 'v-main', ?1, ?2, 'scanner_rename', ?3)",
            params![
                old_inbox.to_string_lossy().to_string(),
                new_inbox.to_string_lossy().to_string(),
                aegis_core::now_ms(),
            ],
        )
        .unwrap();

    let rule = save_folderless_rule(
        store.conn(),
        &FolderlessRuleInput {
            id: None,
            name: "Invoices".into(),
            intent_text: "Route vendor invoices".into(),
            compiled_by: None,
            enabled: Some(true),
            scope_root: Some(old_inbox.to_string_lossy().to_string()),
            match_kinds: vec![],
            match_exts: vec!["pdf".into()],
            match_categories: vec![],
            match_doc_types: vec!["invoice".into()],
            match_keywords: vec![],
            match_orgs: vec![],
            min_age_days: None,
            dest_root: dest.to_string_lossy().to_string(),
            path_template: "{year}".into(),
            rename_template: None,
        },
    )
    .unwrap();

    let report = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(report.rule_issues.len(), 1);
    assert_eq!(report.rule_issues[0].status, HealResolutionStatus::Resolved);
    assert_eq!(
        report.rule_issues[0].suggested_path.as_deref(),
        Some(new_inbox.to_string_lossy().as_ref())
    );

    // Dismissals persist and can be restored.
    let key = report.rule_issues[0].issue_key.clone();
    dismiss_heal_issue(store.conn(), &key, "rule_path", Some("Hold")).unwrap();
    let after_dismiss =
        scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(after_dismiss.rule_issues.len(), 0);
    undismiss_heal_issue(store.conn(), &key).unwrap();

    // Healing rule path updates folderless_rules safely and clears issue.
    heal_folderless_rule_path(
        &mut store,
        &rule.id,
        "scope_root",
        &new_inbox.to_string_lossy(),
    )
    .unwrap();
    let updated_rules = list_folderless_rules(store.conn()).unwrap();
    assert_eq!(
        updated_rules[0].scope_root.as_deref(),
        Some(new_inbox.to_string_lossy().as_ref())
    );

    let final_report =
        scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(final_report.rule_issues.len(), 0);

    let doc = run_doctor(&mut store, false).unwrap();
    assert!(doc.checks.iter().take(3).all(|c| c.passed));
}

#[test]
fn handles_ambiguous_targets_safely_without_auto_patching() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let _a = seed_file(&store, "v-main", tmp.path(), "Alpha\\roadmap.md", "# Alpha Roadmap");
    let _b = seed_file(&store, "v-main", tmp.path(), "Beta\\roadmap.md", "# Beta Roadmap");
    let _src = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Overview.md",
        "Check [Roadmap](missing_dir/roadmap.md).",
    );

    let report = scan_workspace_healing(store.conn(), &HealScanOptions::default(), false).unwrap();
    assert_eq!(report.broken_links.len(), 1);
    assert_eq!(
        report.broken_links[0].status,
        HealResolutionStatus::Ambiguous
    );
    assert_eq!(report.broken_links[0].candidate_paths.len(), 2);

    let err = propose_heal_batch(
        &mut store,
        std::slice::from_ref(&report.broken_links[0].issue_key),
        &[],
        false,
        false,
    )
    .unwrap_err();
    assert!(matches!(err, AegisError::InvalidInput(_)));
}
