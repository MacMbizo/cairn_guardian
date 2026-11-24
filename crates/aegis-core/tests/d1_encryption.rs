//! D1 SQLCipher at-rest encryption, sqlite-vec loading, Windows DPAPI sealing (`R2`),
//! 24-word/hex recovery, `R7` sensitive preview masking, and `R9` offline-only dependency guard.

use aegis_core::contracts::SearchFilters;
use aegis_core::db::SqliteStore;
use aegis_core::db::audit::append_audit_in_tx;
use aegis_core::db::crypto::{
    DbKey, VaultOpenResult, confirm_recovery_backup, is_plaintext_sqlite, open_or_migrate_encrypted,
    recover_vault_with_key, seal_key_to_file, sealed_key_path, unseal_key_from_file,
};
use aegis_core::db::files::{
    DiscoveredFileInput, SENSITIVE_MASK, get_file_detail, get_file_detail_with_options,
    insert_chunk, search_fts, search_fts_with_options, set_file_sensitivity,
    upsert_discovered_file,
};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn seed_volume(conn: &Connection, volume_id: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO volumes (id, mount_point, fs_type, status, last_seen)
         VALUES (?1, 'C:\\', 'ntfs', 'online', 1000)",
        params![volume_id],
    )
    .expect("seed volume");
}

fn seed_file_with_chunk(
    conn: &Connection,
    volume_id: &str,
    path: &str,
    text: &str,
) -> String {
    let file_index = blake3::hash(path.as_bytes()).as_bytes()[..16].to_vec();
    let file_id = upsert_discovered_file(
        conn,
        &DiscoveredFileInput {
            volume_id: volume_id.to_string(),
            has_stable_inode: true,
            case_sensitive: false,
            file_index: Some(file_index),
            nlink: 1,
            atomic_unit_id: None,
            path: PathBuf::from(path),
            size: text.len() as i64,
            mtime: 2000,
            ctime: 1500,
            blake3: None,
            cloud_state: "local".to_string(),
        },
    )
    .expect("upsert file");
    insert_chunk(conn, &file_id, 0, text, 8, false).expect("insert chunk");
    file_id
}

fn f32s_to_le_bytes(vals: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vals.len() * 4);
    for v in vals {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn sqlcipher_encrypts_at_rest_and_rejects_wrong_key() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("encrypted.db");
    let key = DbKey::generate().unwrap();
    let wrong_key = DbKey::generate().unwrap();
    let canary = "AEGIS_D1_SECRET_CANARY_XYZZY_9482";

    {
        let store = SqliteStore::open_encrypted(&db_path, &key).unwrap();
        assert!(store.is_encrypted());

        let (cipher_ver, provider, vec_ver) = store.cipher_info().unwrap();
        assert!(!cipher_ver.is_empty(), "sqlcipher cipher_version must be set");
        assert!(
            provider.to_ascii_lowercase().contains("libtomcrypt"),
            "expected libtomcrypt provider, got {provider}"
        );
        assert!(!vec_ver.is_empty(), "sqlite-vec version must be present");

        seed_volume(store.conn(), "vol_enc");
        seed_file_with_chunk(store.conn(), "vol_enc", r"C:\docs\canary.txt", canary);

        store
            .conn()
            .execute_batch(
                "CREATE VIRTUAL TABLE vec_test USING vec0(embedding float[4]);",
            )
            .unwrap();
        let v1 = f32s_to_le_bytes(&[1.0, 0.0, 0.0, 0.0]);
        let v2 = f32s_to_le_bytes(&[0.0, 1.0, 0.0, 0.0]);
        store
            .conn()
            .execute("INSERT INTO vec_test (rowid, embedding) VALUES (1, ?1)", params![v1])
            .unwrap();
        store
            .conn()
            .execute("INSERT INTO vec_test (rowid, embedding) VALUES (2, ?1)", params![v2])
            .unwrap();

        let query = f32s_to_le_bytes(&[0.98, 0.02, 0.0, 0.0]);
        let (nearest_id, dist): (i64, f64) = store
            .conn()
            .query_row(
                "SELECT rowid, distance FROM vec_test WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                params![query],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(nearest_id, 1);
        assert!(dist < 0.1, "expected small distance, got {dist}");

        store.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    }

    assert!(!is_plaintext_sqlite(&db_path).unwrap());
    let raw_db = std::fs::read(&db_path).unwrap();
    assert!(!contains_subslice(&raw_db, b"SQLite format 3"));
    assert!(
        !contains_subslice(&raw_db, canary.as_bytes()),
        "encrypted database file leaked plaintext chunk text!"
    );

    assert!(
        SqliteStore::open_encrypted(&db_path, &wrong_key).is_err(),
        "opening encrypted DB with wrong key must fail"
    );
    assert!(
        SqliteStore::open(&db_path).is_err(),
        "opening encrypted DB without key must fail"
    );

    let reopened = SqliteStore::open_encrypted(&db_path, &key).unwrap();
    let hits = search_fts(reopened.conn(), canary, &SearchFilters::default(), 10).unwrap();
    assert_eq!(hits.total_hits, 1);
    let q = f32s_to_le_bytes(&[0.0, 1.0, 0.0, 0.0]);
    let second_id: i64 = reopened
        .conn()
        .query_row(
            "SELECT rowid FROM vec_test WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
            params![q],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(second_id, 2);
}

#[test]
fn plaintext_migration_is_lossless_and_removes_plaintext() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("aegis.db");
    let canary = "PLAINTEXT_MIGRATION_SENTINEL_7319";

    {
        let plain = SqliteStore::open(&db_path).unwrap();
        assert!(!plain.is_encrypted());
        seed_volume(plain.conn(), "vol_mig");
        seed_file_with_chunk(plain.conn(), "vol_mig", r"C:\work\budget.md", canary);
        plain
            .conn()
            .execute(
                "INSERT INTO collections (id, kind, name, project_kind, confidence, state, computed_at)
                 VALUES ('col_1', 'project', 'alpha_app', 'software', 0.95, 'active', 1234)",
                [],
            )
            .unwrap();
        append_audit_in_tx(plain.conn(), "test", "seed", "vol_mig", "{}").unwrap();
        plain.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    }

    assert!(is_plaintext_sqlite(&db_path).unwrap());

    {
        let res = open_or_migrate_encrypted(&db_path).unwrap();
        let (store, _key, meta, migrated) = match res {
            VaultOpenResult::Ready {
                store,
                key,
                meta,
                migrated,
            } => (store, key, meta, migrated),
            VaultOpenResult::NeedsRecoveryKey { reason } => {
                panic!("unexpected NeedsRecoveryKey after migration: {reason}");
            }
        };
        assert!(migrated);
        assert!(meta.migrated_from_plaintext_at.is_some());
        assert!(meta.recovery_confirmed_at.is_none());
        assert!(store.is_encrypted());

        for (tbl, expected) in [("files", 1i64), ("chunks", 1), ("collections", 1), ("audit_log", 1)]
        {
            let count: i64 = store
                .conn()
                .query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, expected, "row count mismatch after migration for {tbl}");
        }

        let hits = search_fts(store.conn(), canary, &SearchFilters::default(), 10).unwrap();
        assert_eq!(hits.total_hits, 1);

        let quick: String = store
            .conn()
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(quick.to_ascii_lowercase(), "ok");
    }

    assert!(!is_plaintext_sqlite(&db_path).unwrap());
    let raw_after = std::fs::read(&db_path).unwrap();
    assert!(!contains_subslice(&raw_after, b"SQLite format 3"));
    assert!(!contains_subslice(&raw_after, canary.as_bytes()));

    // Normal unlink (`R8`): no temporary migration artifacts left behind.
    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !entries.iter().any(|n| n.contains(".plaintext.bak") || n.contains(".encrypting")),
        "leftover temporary files beside migrated DB: {entries:?}"
    );

    // Unattended DPAPI reopen after migration works cleanly with migrated = false.
    let second = open_or_migrate_encrypted(&db_path).unwrap();
    match second {
        VaultOpenResult::Ready {
            store,
            migrated,
            ..
        } => {
            assert!(!migrated);
            assert!(store.is_encrypted());
            let hits = search_fts(store.conn(), canary, &SearchFilters::default(), 10).unwrap();
            assert_eq!(hits.total_hits, 1);
        }
        VaultOpenResult::NeedsRecoveryKey { reason } => {
            panic!("expected unattended reopen, got locked: {reason}");
        }
    }
}

#[test]
fn recovery_key_24_words_and_hex_recover_without_dpapi() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("aegis.db");

    let (words, hex, key_clone) = {
        let res = open_or_migrate_encrypted(&db_path).unwrap();
        let (store, key, _meta, _) = match res {
            VaultOpenResult::Ready {
                store,
                key,
                meta,
                migrated,
            } => (store, key, meta, migrated),
            VaultOpenResult::NeedsRecoveryKey { reason } => panic!("unexpected lock: {reason}"),
        };
        seed_volume(store.conn(), "vol_rec");
        seed_file_with_chunk(store.conn(), "vol_rec", r"C:\notes\recovery.txt", "RECOVERY_OK_5512");

        let words = key.to_recovery_words_vec().unwrap();
        assert_eq!(words.len(), 24);
        let hex = key.to_recovery_hex().to_string();
        assert_eq!(hex.chars().filter(|c| c.is_ascii_hexdigit()).count(), 64);

        // Invalid word and invalid BIP-39 checksum must both fail validation deterministically.
        let mut unknown_words = words.clone();
        unknown_words[0] = "notabip39word".into();
        assert!(DbKey::from_recovery_input(&unknown_words.join(" ")).is_err());

        // For a fixed 23-word prefix, at most 8 of 2048 English words produce a valid 8-bit checksum,
        // so at least one of 9 candidate 24th words is mathematically guaranteed to fail checksum.
        let bad_checksum = [
            "abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract", "absurd",
        ]
        .into_iter()
        .find_map(|cand| {
            let mut probe = words.clone();
            probe[23] = cand.into();
            if DbKey::from_recovery_input(&probe.join(" ")).is_err() {
                Some(probe.join(" "))
            } else {
                None
            }
        })
        .expect("at least one candidate 24th word must fail BIP-39 checksum");
        assert!(DbKey::from_recovery_input(&bad_checksum).is_err());

        // Spot-check confirmation rules.
        assert!(confirm_recovery_backup(&db_path, &key, &[(1, words[0].clone())]).is_err());
        assert!(
            confirm_recovery_backup(&db_path, &key, &[(1, words[0].clone()), (1, words[0].clone())])
                .is_err()
        );
        assert!(
            confirm_recovery_backup(&db_path, &key, &[(1, "wrongword".into()), (2, words[1].clone())])
                .is_err()
        );
        let confirmed = confirm_recovery_backup(
            &db_path,
            &key,
            &[(1, words[0].clone()), (24, words[23].clone())],
        )
        .unwrap();
        assert!(confirmed.recovery_confirmed_at.is_some());

        (words, hex, key)
    };

    // Simulate missing DPAPI key (e.g., new Windows install / moved DB).
    let dpapi_file = sealed_key_path(&db_path);
    std::fs::remove_file(&dpapi_file).unwrap();

    match open_or_migrate_encrypted(&db_path).unwrap() {
        VaultOpenResult::NeedsRecoveryKey { .. } => {}
        VaultOpenResult::Ready { .. } => panic!("vault must lock when DPAPI key is missing"),
    }

    // Recover using 24-word mnemonic.
    {
        let (store, recovered_key, meta) = recover_vault_with_key(&db_path, &words.join(" ")).unwrap();
        assert_eq!(recovered_key.fingerprint(), key_clone.fingerprint());
        assert!(meta.recovery_confirmed_at.is_some());
        assert!(dpapi_file.is_file(), "recovery should re-seal DPAPI key for this account");
        let hits = search_fts(store.conn(), "RECOVERY_OK_5512", &SearchFilters::default(), 10).unwrap();
        assert_eq!(hits.total_hits, 1);
    }

    // Remove DPAPI again and recover using formatted 64-hex key.
    std::fs::remove_file(&dpapi_file).unwrap();
    {
        let (store, recovered_key, _) = recover_vault_with_key(&db_path, &hex).unwrap();
        assert_eq!(recovered_key.fingerprint(), key_clone.fingerprint());
        let hits = search_fts(store.conn(), "RECOVERY_OK_5512", &SearchFilters::default(), 10).unwrap();
        assert_eq!(hits.total_hits, 1);
    }

    // Unattended DPAPI reopen now succeeds.
    match open_or_migrate_encrypted(&db_path).unwrap() {
        VaultOpenResult::Ready { store, .. } => {
            assert!(store.is_encrypted());
        }
        VaultOpenResult::NeedsRecoveryKey { reason } => {
            panic!("unattended reopen should succeed after recovery re-seals DPAPI: {reason}");
        }
    }
}

#[test]
fn dpapi_seal_unseal_roundtrip_and_tamper_rejection() {
    let dir = TempDir::new().unwrap();
    let sealed_path = dir.path().join("aegis.key.dpapi");
    let key = DbKey::generate().unwrap();

    seal_key_to_file(&key, &sealed_path).unwrap();
    let unsealed = unseal_key_from_file(&sealed_path).unwrap();
    assert_eq!(unsealed.as_bytes(), key.as_bytes());

    let mut bytes = std::fs::read(&sealed_path).unwrap();
    assert!(bytes.len() > 24);
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    std::fs::write(&sealed_path, &bytes).unwrap();
    assert!(
        unseal_key_from_file(&sealed_path).is_err(),
        "corrupted DPAPI ciphertext must fail unseal"
    );

    // Also reject corrupt framing magic header.
    seal_key_to_file(&key, &sealed_path).unwrap();
    let mut bad_hdr = std::fs::read(&sealed_path).unwrap();
    bad_hdr[0] = b'X';
    std::fs::write(&sealed_path, &bad_hdr).unwrap();
    assert!(
        unseal_key_from_file(&sealed_path).is_err(),
        "corrupted key header magic must fail unseal"
    );
}

#[test]
fn sensitive_files_mask_previews_until_unlocked_r7() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("r7.db");
    let key = DbKey::generate().unwrap();
    let store = SqliteStore::open_encrypted(&db_path, &key).unwrap();
    seed_volume(store.conn(), "vol_r7");

    let normal_id = seed_file_with_chunk(
        store.conn(),
        "vol_r7",
        r"C:\docs\public_summary.txt",
        "taxpayer_alpha_99 public overview notes",
    );
    let secret_id = seed_file_with_chunk(
        store.conn(),
        "vol_r7",
        r"C:\docs\private_return.txt",
        "taxpayer_alpha_99 SSN 123-45-6789 confidential",
    );

    assert!(set_file_sensitivity(store.conn(), &secret_id, "top_secret").is_err());
    set_file_sensitivity(store.conn(), &secret_id, "confidential").unwrap();

    // Locked sensitive view (`R7`): full index remains searchable, but sensitive snippet/preview is masked.
    let locked_page =
        search_fts(store.conn(), "taxpayer_alpha_99", &SearchFilters::default(), 20).unwrap();
    assert_eq!(locked_page.total_hits, 2);
    let normal_hit = locked_page.hits.iter().find(|h| h.file_id == normal_id).unwrap();
    let secret_hit = locked_page.hits.iter().find(|h| h.file_id == secret_id).unwrap();
    assert!(normal_hit.snippet.contains("public overview"));
    assert_eq!(secret_hit.snippet, SENSITIVE_MASK);

    let locked_detail = get_file_detail(store.conn(), &secret_id).unwrap();
    assert_eq!(locked_detail.chunks_preview, vec![SENSITIVE_MASK.to_string()]);

    // Unlocked sensitive view: real snippets & chunk previews are revealed.
    let unlocked_page =
        search_fts_with_options(store.conn(), "taxpayer_alpha_99", &SearchFilters::default(), 20, true)
            .unwrap();
    let unlocked_hit = unlocked_page.hits.iter().find(|h| h.file_id == secret_id).unwrap();
    assert!(unlocked_hit.snippet.contains("123-45-6789"));

    let unlocked_detail = get_file_detail_with_options(store.conn(), &secret_id, true).unwrap();
    assert!(
        unlocked_detail
            .chunks_preview
            .iter()
            .any(|p| p.contains("123-45-6789"))
    );
}

#[derive(Debug, Clone)]
struct LockPkg {
    name: String,
    version: String,
    deps: Vec<String>,
}

fn parse_cargo_lock(text: &str) -> Vec<LockPkg> {
    let mut pkgs = Vec::new();
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut deps = Vec::new();
    let mut in_deps = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line == "[[package]]" {
            if let (Some(n), Some(v)) = (name.take(), version.take()) {
                pkgs.push(LockPkg {
                    name: n,
                    version: v,
                    deps: std::mem::take(&mut deps),
                });
            }
            in_deps = false;
            continue;
        }
        if let Some(rest) = line.strip_prefix("name = \"") {
            name = Some(rest.trim_end_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("version = \"") {
            version = Some(rest.trim_end_matches('"').to_string());
        } else if line == "dependencies = [" {
            in_deps = true;
        } else if in_deps {
            if line == "]" {
                in_deps = false;
            } else if let Some(val) = line
                .trim_end_matches(',')
                .strip_prefix('"')
                .and_then(|q| q.strip_suffix('"'))
            {
                deps.push(val.to_string());
            }
        }
    }
    if let (Some(n), Some(v)) = (name, version) {
        pkgs.push(LockPkg {
            name: n,
            version: v,
            deps,
        });
    }
    pkgs
}

#[test]
fn r9_forbids_networking_crates_in_engine_and_models() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lock_path = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("Cargo.lock");
    let lock_text = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("failed reading {}: {e}", lock_path.display()));
    let pkgs = parse_cargo_lock(&lock_text);
    assert!(!pkgs.is_empty(), "Cargo.lock parsed 0 packages");

    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (idx, p) in pkgs.iter().enumerate() {
        by_name.entry(p.name.as_str()).or_default().push(idx);
    }

    let mut queue = VecDeque::new();
    let mut visited_idxs = BTreeSet::new();
    for root in ["aegis-core", "aegis-embed", "aegis-llm"] {
        let idxs = by_name
            .get(root)
            .unwrap_or_else(|| panic!("missing root crate {root} in Cargo.lock"));
        for &i in idxs {
            if visited_idxs.insert(i) {
                queue.push_back(i);
            }
        }
    }

    let mut transitive_names = BTreeSet::new();
    while let Some(idx) = queue.pop_front() {
        transitive_names.insert(pkgs[idx].name.as_str());
        for dep in &pkgs[idx].deps {
            let mut parts = dep.split_whitespace();
            let dep_name = parts.next().unwrap();
            let dep_ver = parts.next();
            if let Some(cands) = by_name.get(dep_name) {
                for &ci in cands {
                    if dep_ver.is_none_or(|v| pkgs[ci].version == v) && visited_idxs.insert(ci) {
                        queue.push_back(ci);
                    }
                }
            }
        }
    }

    let forbidden = [
        "reqwest",
        "ureq",
        "hyper",
        "hyper-util",
        "h2",
        "http-body",
        "tower-http",
        "rustls",
        "native-tls",
        "tokio-rustls",
        "tokio-native-tls",
        "openssl",
        "openssl-sys",
        "curl",
        "curl-sys",
        "surf",
        "isahc",
        "attohttpc",
        "async-h1",
        "tonic",
        "tungstenite",
        "tokio-tungstenite",
        "hf-hub",
        "ort",
        "ort-sys",
        "socket2",
        "quinn",
        "hickory-resolver",
    ];

    let violations: Vec<&&str> = forbidden
        .iter()
        .filter(|name| transitive_names.contains(**name))
        .collect();
    assert!(
        violations.is_empty(),
        "R9 violation: forbidden networking crates found in aegis-core/aegis-embed graph: {violations:?}"
    );
}
