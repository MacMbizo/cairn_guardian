//! Regression tests for the safety/correctness fixes: disk/DB consistency, rollback completion,
//! non-overwriting copy-back, per-item restore/purge, scan sweep/rehash/ignore, and dedupe proposals.

use aegis_core::contracts::{
    ActionType, ConflictResolutionChoice, ProposedActionInput, RefilePlan, RollbackConflictKind,
    SearchFilters, TargetKind,
};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{
    approve_batch, execute_batch, exact_dedupe_report, propose_batch, propose_dupe_quarantine,
    resolve_rollback_conflict, rollback_batch,
};
use aegis_core::db::files::{normalize_path, path_is_within, search_fts};
use aegis_core::quarantine::{list_quarantine, purge_entries, restore_entries};
use aegis_core::scanner::scan_root;
use rusqlite::params;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    qdir: PathBuf,
    store: SqliteStore,
}

/// Root under a temp dir, scanned once, with quarantine pinned inside the temp dir.
fn fixture(files: &[(&str, &str)]) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    for (rel, content) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    let qdir = tmp.path().join(".aegis-quarantine");
    let store = SqliteStore::open_in_memory().unwrap();
    store.add_root(&root).unwrap();
    let mut fx = Fixture {
        _tmp: tmp,
        root,
        qdir,
        store,
    };
    rescan(&mut fx);
    fx
}

fn rescan(fx: &mut Fixture) {
    scan_root(&mut fx.store, &fx.root, &[1, 2]).unwrap();
    fx.store
        .conn()
        .execute(
            "UPDATE volumes SET quarantine_dir = ?1",
            params![fx.qdir.to_string_lossy().to_string()],
        )
        .unwrap();
}

fn file_id(store: &SqliteStore, path: &Path) -> String {
    let (_, ph) = normalize_path(path, false);
    store
        .conn()
        .query_row(
            "SELECT id FROM files WHERE path_hash = ?1 AND deleted_at IS NULL",
            params![ph.as_slice()],
            |r| r.get(0),
        )
        .unwrap()
}

fn action(
    at: ActionType,
    fid: Option<String>,
    tk: TargetKind,
    from: &Path,
    to: Option<&Path>,
) -> ProposedActionInput {
    ProposedActionInput {
        agent: None,
        action_type: at,
        target_kind: tk,
        file_id: fid,
        atomic_unit_id: None,
        from_path: from.to_string_lossy().to_string(),
        to_path: to.map(|p| p.to_string_lossy().to_string()),
        reason: "test".into(),
        confidence: 1.0,
        evidence_json: "[]".into(),
    }
}

fn move_plan(store: &SqliteStore, pairs: &[(&Path, &Path)]) -> RefilePlan {
    RefilePlan {
        origin: "simulator".into(),
        actions: pairs
            .iter()
            .map(|(from, to)| {
                action(
                    ActionType::Move,
                    Some(file_id(store, from)),
                    TargetKind::File,
                    from,
                    Some(to),
                )
            })
            .collect(),
    }
}

fn batch_status(store: &SqliteStore, batch: &str) -> String {
    store
        .conn()
        .query_row(
            "SELECT status FROM action_batches WHERE id = ?1",
            params![batch],
            |r| r.get(0),
        )
        .unwrap()
}

fn action_status(store: &SqliteStore, batch: &str) -> Vec<String> {
    let mut stmt = store
        .conn()
        .prepare("SELECT status FROM actions WHERE batch_id = ?1 ORDER BY created_at, id")
        .unwrap();
    stmt.query_map(params![batch], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn path_prefix_matching_respects_component_boundaries() {
    assert!(path_is_within(r"C:\data\a.txt", r"C:\data", false));
    assert!(path_is_within(r"\\?\C:\Data\a.txt", r"c:\data\", false));
    assert!(!path_is_within(r"C:\database\a.txt", r"C:\data", false));
    assert!(path_is_within(r"C:\x\y", r"C:\", false));
    assert!(path_is_within("/home/u/f", "/", true));
    assert!(!path_is_within("/home/U/f", "/home/u", true));
}

#[test]
fn execute_requires_approval_and_rerun_does_not_corrupt_status() {
    let mut fx = fixture(&[("a.txt", "alpha")]);
    let a = fx.root.join("a.txt");
    let dest = fx.root.join("sorted").join("a.txt");
    let plan = move_plan(&fx.store, &[(&a, &dest)]);
    let batch = propose_batch(&mut fx.store, &plan).unwrap();

    let err = execute_batch(&mut fx.store, &batch);
    assert!(err.is_err(), "execute must refuse an unapproved batch");
    assert!(a.exists());
    assert_eq!(batch_status(&fx.store, &batch), "draft");

    approve_batch(&mut fx.store, &batch, None).unwrap();
    let res = execute_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(res.executed_count, 1);
    assert!(dest.exists());

    // Second execute is refused instead of rewriting the executed batch as 'failed'.
    assert!(execute_batch(&mut fx.store, &batch).is_err());
    assert_eq!(batch_status(&fx.store, &batch), "executed");
    // Re-approving an executed batch is refused too.
    assert!(approve_batch(&mut fx.store, &batch, None).is_err());
}

#[test]
fn failed_commit_undoes_the_disk_change() {
    let mut fx = fixture(&[("keep.txt", "k"), ("a.txt", "alpha")]);
    let a = fx.root.join("a.txt");
    let dest = fx.root.join("sorted").join("a.txt");
    let plan = move_plan(&fx.store, &[(&a, &dest)]);
    let batch = propose_batch(&mut fx.store, &plan).unwrap();
    approve_batch(&mut fx.store, &batch, None).unwrap();

    // Inject a DB failure exactly when the action is recorded as executed.
    fx.store
        .conn()
        .execute_batch(
            "CREATE TRIGGER inject_fail BEFORE UPDATE OF status ON actions
             WHEN NEW.status = 'executed' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();

    let res = execute_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(res.executed_count, 0);
    assert_eq!(res.failed_count, 1);
    assert!(a.exists(), "disk move must be undone when its commit fails");
    assert!(!dest.exists());
    let msg = &res.actions[0].error_message;
    assert!(msg.as_deref().unwrap_or("").contains("undone"), "{msg:?}");
    // Index still points at the original path.
    let path: String = fx
        .store
        .conn()
        .query_row(
            "SELECT path FROM files WHERE id = ?1",
            params![res.actions[0].file_id.clone().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(path.ends_with("a.txt") && !path.contains("sorted"));
}

#[test]
fn quarantined_directory_records_real_size() {
    let mut fx = fixture(&[
        ("proj/package.json", "{}"),
        ("proj/node_modules/pkg/index.js", "module.exports = 1234567890;"),
        ("proj/node_modules/pkg/big.bin", &"x".repeat(4096)),
    ]);
    let nm = fx.root.join("proj").join("node_modules");
    let plan = RefilePlan {
        origin: "dedupe".into(),
        actions: vec![action(ActionType::Quarantine, None, TargetKind::Dir, &nm, None)],
    };
    let batch = propose_batch(&mut fx.store, &plan).unwrap();
    approve_batch(&mut fx.store, &batch, None).unwrap();
    let res = execute_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(res.executed_count, 1, "{:?}", res.actions[0].error_message);

    let entries = list_quarantine(fx.store.conn(), None).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0].size_bytes >= 4096,
        "dir quarantine must record its size, got {}",
        entries[0].size_bytes
    );
    assert!(entries[0].quarantine_path.starts_with(&*fx.qdir.to_string_lossy()));
}

#[test]
fn rollback_reverts_clean_actions_once_conflicts_are_resolved() {
    let mut fx = fixture(&[("m.txt", "modify me"), ("c.txt", "clean one")]);
    let m = fx.root.join("m.txt");
    let c = fx.root.join("c.txt");
    let m_to = fx.root.join("arch").join("m.txt");
    let c_to = fx.root.join("arch").join("c.txt");
    let plan = move_plan(&fx.store, &[(&m, &m_to), (&c, &c_to)]);
    let batch = propose_batch(&mut fx.store, &plan).unwrap();
    approve_batch(&mut fx.store, &batch, None).unwrap();
    execute_batch(&mut fx.store, &batch).unwrap();

    fs::write(&m_to, "edited after move").unwrap();
    let rb = rollback_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(rb.conflicts.len(), 1);
    assert_eq!(rb.reverted_count, 0);
    assert!(c_to.exists() && !c.exists(), "nothing moves until conflicts are resolved");

    let res = resolve_rollback_conflict(
        &mut fx.store,
        &rb.conflicts[0].action_id,
        ConflictResolutionChoice::RevertKeepingEdits,
    )
    .unwrap();
    assert!(res.conflicts.is_empty());
    assert_eq!(res.reverted_count, 2, "resolving the last conflict must finish the batch");
    assert!(c.exists() && !c_to.exists(), "clean action must be reverted too");
    assert_eq!(fs::read_to_string(&m).unwrap(), "edited after move");
    assert_eq!(batch_status(&fx.store, &batch), "rolled_back");
    assert!(action_status(&fx.store, &batch).iter().all(|s| s == "rolled_back"));
}

#[test]
fn skipped_conflict_leaves_batch_partial_but_finishes_the_rest() {
    let mut fx = fixture(&[("gone.txt", "will vanish"), ("c.txt", "clean")]);
    let g = fx.root.join("gone.txt");
    let c = fx.root.join("c.txt");
    let g_to = fx.root.join("arch").join("gone.txt");
    let c_to = fx.root.join("arch").join("c.txt");
    let plan = move_plan(&fx.store, &[(&g, &g_to), (&c, &c_to)]);
    let batch = propose_batch(&mut fx.store, &plan).unwrap();
    approve_batch(&mut fx.store, &batch, None).unwrap();
    execute_batch(&mut fx.store, &batch).unwrap();

    fs::remove_file(&g_to).unwrap();
    let rb = rollback_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(rb.conflicts[0].conflict, RollbackConflictKind::MissingTarget);
    let res = resolve_rollback_conflict(
        &mut fx.store,
        &rb.conflicts[0].action_id,
        ConflictResolutionChoice::Skip,
    )
    .unwrap();
    assert!(res.conflicts.is_empty());
    assert!(c.exists());
    assert_eq!(batch_status(&fx.store, &batch), "partial");

    // Resolving an action that has no pending conflict is refused.
    assert!(
        resolve_rollback_conflict(&mut fx.store, &rb.conflicts[0].action_id, ConflictResolutionChoice::Skip)
            .is_err()
    );
}

#[test]
fn copy_back_never_overwrites_an_occupied_original() {
    let mut fx = fixture(&[("doc.txt", "original")]);
    let d = fx.root.join("doc.txt");
    let d_to = fx.root.join("arch").join("doc.txt");
    let plan = move_plan(&fx.store, &[(&d, &d_to)]);
    let batch = propose_batch(&mut fx.store, &plan).unwrap();
    approve_batch(&mut fx.store, &batch, None).unwrap();
    execute_batch(&mut fx.store, &batch).unwrap();

    // Edited at the destination AND a new file now sits at the original path.
    fs::write(&d_to, "edited").unwrap();
    fs::write(&d, "brand new unrelated file").unwrap();
    let rb = rollback_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(rb.conflicts[0].conflict, RollbackConflictKind::Modified);

    resolve_rollback_conflict(
        &mut fx.store,
        &rb.conflicts[0].action_id,
        ConflictResolutionChoice::CopyBackKeepingBoth,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&d).unwrap(),
        "brand new unrelated file",
        "copy-back must not overwrite the file now at the original path"
    );
    assert_eq!(
        fs::read_to_string(fx.root.join("doc (2).txt")).unwrap(),
        "edited"
    );
    assert!(d_to.exists(), "keep-both leaves the moved copy in place");

    // RevertKeepingEdits onto an occupied path is refused rather than overwriting.
    let mut fx2 = fixture(&[("x.txt", "x")]);
    let x = fx2.root.join("x.txt");
    let x_to = fx2.root.join("arch").join("x.txt");
    let plan2 = move_plan(&fx2.store, &[(&x, &x_to)]);
    let b2 = propose_batch(&mut fx2.store, &plan2).unwrap();
    approve_batch(&mut fx2.store, &b2, None).unwrap();
    execute_batch(&mut fx2.store, &b2).unwrap();
    fs::write(&x_to, "edited").unwrap();
    fs::write(&x, "occupant").unwrap();
    let rb2 = rollback_batch(&mut fx2.store, &b2).unwrap();
    assert!(
        resolve_rollback_conflict(
            &mut fx2.store,
            &rb2.conflicts[0].action_id,
            ConflictResolutionChoice::RevertKeepingEdits
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(&x).unwrap(), "occupant");
}

fn quarantine_files(fx: &mut Fixture, names: &[&str]) -> Vec<String> {
    let actions = names
        .iter()
        .map(|n| {
            let p = fx.root.join(n);
            action(
                ActionType::Quarantine,
                Some(file_id(&fx.store, &p)),
                TargetKind::File,
                &p,
                None,
            )
        })
        .collect();
    let batch = propose_batch(
        &mut fx.store,
        &RefilePlan {
            origin: "dedupe".into(),
            actions,
        },
    )
    .unwrap();
    approve_batch(&mut fx.store, &batch, None).unwrap();
    let res = execute_batch(&mut fx.store, &batch).unwrap();
    assert_eq!(res.executed_count, names.len());
    let mut ids: Vec<(String, String)> = list_quarantine(fx.store.conn(), None)
        .unwrap()
        .into_iter()
        .map(|q| (q.original_path, q.id))
        .collect();
    ids.sort();
    names
        .iter()
        .map(|n| {
            ids.iter()
                .find(|(p, _)| p.ends_with(n))
                .unwrap()
                .1
                .clone()
        })
        .collect()
}

#[test]
fn restore_failure_on_one_item_does_not_strand_the_others() {
    let mut fx = fixture(&[("r1.txt", "one"), ("r2.txt", "two")]);
    let ids = quarantine_files(&mut fx, &["r1.txt", "r2.txt"]);

    // r1's quarantined payload vanishes externally.
    let entries = list_quarantine(fx.store.conn(), None).unwrap();
    let r1_q = entries.iter().find(|q| q.id == ids[0]).unwrap();
    fs::remove_file(&r1_q.quarantine_path).unwrap();

    let res = restore_entries(&mut fx.store, &ids);
    assert!(res.is_err(), "the failed item must be reported");
    assert!(fx.root.join("r2.txt").exists(), "the good item is restored");
    let remaining = list_quarantine(fx.store.conn(), None).unwrap();
    assert_eq!(remaining.len(), 1, "only the failed item stays listed");
    assert_eq!(remaining[0].id, ids[0]);
}

#[test]
fn restore_commit_failure_moves_the_item_back_into_quarantine() {
    let mut fx = fixture(&[("r.txt", "payload")]);
    let ids = quarantine_files(&mut fx, &["r.txt"]);
    let qpath = list_quarantine(fx.store.conn(), None).unwrap()[0]
        .quarantine_path
        .clone();
    fx.store
        .conn()
        .execute_batch(
            "CREATE TRIGGER inject_fail BEFORE UPDATE OF restored_at ON quarantine_entries
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
    assert!(restore_entries(&mut fx.store, &ids).is_err());
    assert!(Path::new(&qpath).exists(), "item must be back in quarantine");
    assert!(!fx.root.join("r.txt").exists());
    assert_eq!(list_quarantine(fx.store.conn(), None).unwrap().len(), 1);
}

#[test]
fn purge_missing_payload_is_recorded_and_others_still_purge() {
    let mut fx = fixture(&[("p1.txt", "one"), ("p2.txt", "two")]);
    let ids = quarantine_files(&mut fx, &["p1.txt", "p2.txt"]);
    let entries = list_quarantine(fx.store.conn(), None).unwrap();
    for q in &entries {
        if q.id == ids[0] {
            fs::remove_file(&q.quarantine_path).unwrap();
        }
    }
    purge_entries(&mut fx.store, &ids, true).unwrap();
    assert!(list_quarantine(fx.store.conn(), None).unwrap().is_empty());
    for q in &entries {
        assert!(!Path::new(&q.quarantine_path).exists());
    }
}

#[test]
fn rescan_retires_deleted_files_and_refreshes_changed_text() {
    let mut fx = fixture(&[
        ("keep.md", "stable zebra content"),
        ("gone.md", "ephemeral walrus content"),
        ("edit.md", "old pelican words"),
    ]);
    let keep = fx.root.join("keep.md");
    let keep_hash_before: Vec<u8> = fx
        .store
        .conn()
        .query_row(
            "SELECT blake3 FROM files WHERE id = ?1",
            params![file_id(&fx.store, &keep)],
            |r| r.get(0),
        )
        .unwrap();

    fs::remove_file(fx.root.join("gone.md")).unwrap();
    // Different length so size changes -> must be re-hashed and re-chunked.
    fs::write(fx.root.join("edit.md"), "new flamingo words here").unwrap();
    rescan(&mut fx);

    let live_gone: i64 = fx
        .store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM files WHERE name = 'gone.md' AND deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live_gone, 0, "deleted file must leave the live index");
    let f = SearchFilters::default();
    assert!(search_fts(fx.store.conn(), "walrus", &f, 10).unwrap().hits.is_empty());
    assert!(search_fts(fx.store.conn(), "pelican", &f, 10).unwrap().hits.is_empty());
    assert_eq!(search_fts(fx.store.conn(), "flamingo", &f, 10).unwrap().hits.len(), 1);
    assert_eq!(search_fts(fx.store.conn(), "zebra", &f, 10).unwrap().hits.len(), 1);

    let keep_hash_after: Vec<u8> = fx
        .store
        .conn()
        .query_row(
            "SELECT blake3 FROM files WHERE id = ?1",
            params![file_id(&fx.store, &keep)],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(keep_hash_before, keep_hash_after);
}

#[test]
fn ignore_rules_saved_from_settings_are_applied() {
    let mut fx = fixture(&[("secret.log", "classified heron"), ("note.md", "public heron")]);
    // The IPC layer stores ignore rules JSON-encoded.
    fx.store
        .set_setting(
            "ignore.global",
            "global",
            &serde_json::to_string("*.log\n").unwrap(),
        )
        .unwrap();
    rescan(&mut fx);
    let hits = search_fts(fx.store.conn(), "heron", &SearchFilters::default(), 10).unwrap();
    assert_eq!(hits.hits.len(), 1);
    assert!(hits.hits[0].path.ends_with("note.md"));
}

#[test]
fn dedupe_cluster_can_be_proposed_for_quarantine() {
    // Three copies: one may go, leaving the minimum of two.
    let mut fx = fixture(&[
        ("a/dup.txt", "same bytes"),
        ("b/c/dup.txt", "same bytes"),
        ("d/dup.txt", "same bytes"),
    ]);
    let report = exact_dedupe_report(fx.store.conn()).unwrap();
    assert_eq!(report.clusters.len(), 1);
    let cl = &report.clusters[0];
    let drop_one = vec![cl.keeper_file_id.clone(), cl.member_file_ids[1].clone()];
    let batch = propose_dupe_quarantine(&mut fx.store, &cl.keeper_file_id, &drop_one).unwrap();
    let statuses = action_status(&fx.store, &batch);
    assert_eq!(statuses, vec!["proposed".to_string()], "only the non-keeper is proposed");

    // Removing every extra copy would leave one: refused.
    assert!(propose_dupe_quarantine(&mut fx.store, &cl.keeper_file_id, &cl.member_file_ids).is_err());
}

#[cfg(windows)]
#[test]
fn junctions_are_links_but_plain_dirs_are_not() {
    use aegis_core::scanner::os_adapter::inspect_path_metadata;
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("target_dir");
    let link = tmp.path().join("junction");
    fs::create_dir_all(&target).unwrap();
    let ok = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(!inspect_path_metadata(&target).unwrap().unwrap().is_symlink);
    if ok {
        assert!(inspect_path_metadata(&link).unwrap().unwrap().is_symlink);
    }
}
