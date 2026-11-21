//! Drives & Duplicates: fingerprinting, grouped duplicates, safety rules and decisions.

mod common;

use aegis_core::db::SqliteStore;
use aegis_core::drives::{FakeVolumeProvider, sync_mounted};
use aegis_core::fingerprint::run_fingerprint;
use std::sync::atomic::AtomicBool;

const KIB: usize = 1024;

fn setup() -> (tempfile::TempDir, SqliteStore, FakeVolumeProvider) {
    (
        tempfile::TempDir::new().unwrap(),
        SqliteStore::open_in_memory().unwrap(),
        FakeVolumeProvider::new(),
    )
}

#[test]
fn unique_sizes_are_never_read() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    common::put(&a.mount_point, "a", &[1u8; 10]);
    common::put(&a.mount_point, "b", &[1u8; 11]);
    common::put(&a.mount_point, "c", &[1u8; 12]);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.size_candidates, 0);
    assert_eq!(stats.quick_hashed, 0);
    assert_eq!(stats.full_hashed, 0);
}

#[test]
fn same_size_different_content_stops_at_quick_hash() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(300 * KIB, 1);
    let mut other = body.clone();
    other[0] ^= 0xff;
    common::put(&a.mount_point, "one.bin", &body);
    common::put(&a.mount_point, "two.bin", &other);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.size_candidates, 2);
    assert_eq!(stats.quick_hashed, 2);
    assert_eq!(stats.full_hashed, 0);
}

#[test]
fn same_size_same_edges_different_middle_gets_full_hash() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(300 * KIB, 2);
    let mut other = body.clone();
    other[150 * KIB] ^= 0xff; // outside the first and last 64 KiB
    let p1 = common::put(&a.mount_point, "one.bin", &body);
    let p2 = common::put(&a.mount_point, "two.bin", &other);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.full_hashed, 2);
    let (q1, b1) = common::hashes_at(&store, &p1);
    let (q2, b2) = common::hashes_at(&store, &p2);
    assert_eq!(q1, q2, "edges are identical");
    assert!(b1.is_some() && b2.is_some());
    assert_ne!(b1, b2);
    assert_eq!(b1.unwrap(), blake3::hash(&body).as_bytes().to_vec());
}

#[test]
fn small_files_get_full_hash_from_quick_pass() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(KIB, 3);
    let p1 = common::put(&a.mount_point, "x.txt", &body);
    let p2 = common::put(&a.mount_point, "sub/x.txt", &body);
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.full_hashed, 0);
    let expected = blake3::hash(&body).as_bytes().to_vec();
    assert_eq!(common::hashes_at(&store, &p1).1, Some(expected.clone()));
    assert_eq!(common::hashes_at(&store, &p2).1, Some(expected));
}

#[test]
fn offline_partner_triggers_full_hash_of_online_file() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(300 * KIB, 4);
    common::put(&a.mount_point, "x.bin", &body);
    common::put(&a.mount_point, "copy/x.bin", &body);
    common::plug_and_catalog(&mut store, &provider, &a);
    run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();

    provider.unplug(&a.mount_point);
    sync_mounted(&mut store, &provider).unwrap();
    let b = common::fake_drive(&tmp, "B", "0002");
    let pb = common::put(&b.mount_point, "x.bin", &body);
    common::plug_and_catalog(&mut store, &provider, &b);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.quick_hashed, 1, "only B's new file is read");
    assert_eq!(stats.full_hashed, 1);
    assert_eq!(stats.skipped_offline, 0, "A's rows were already fingerprinted");
    assert_eq!(
        common::hashes_at(&store, &pb).1,
        Some(blake3::hash(&body).as_bytes().to_vec())
    );
}

#[test]
fn placeholders_are_never_opened() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    let body = common::noise(2 * KIB, 5);
    let p1 = common::put(&a.mount_point, "cloud.bin", &body);
    common::put(&a.mount_point, "local.bin", &body);
    common::put(&a.mount_point, "local2.bin", &body);
    common::plug_and_catalog(&mut store, &provider, &a);
    store
        .conn()
        .execute(
            "UPDATE files SET cloud_state = 'placeholder_remote' WHERE path = ?1",
            [common::canon(&p1)],
        )
        .unwrap();
    std::fs::remove_file(&p1).unwrap(); // reading it would fail loudly
    let stats = run_fingerprint(&mut store, &AtomicBool::new(false)).unwrap();
    assert_eq!(stats.skipped_placeholder, 1);
    let (q, _) = store
        .conn()
        .query_row(
            "SELECT quick_hash, blake3 FROM files WHERE cloud_state = 'placeholder_remote'",
            [],
            |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .unwrap();
    assert!(q.is_none());
}

#[test]
fn cancel_returns_partial_stats() {
    let (tmp, mut store, provider) = setup();
    let a = common::fake_drive(&tmp, "A", "0001");
    for i in 0..5 {
        common::put(&a.mount_point, &format!("f{i}.bin"), &common::noise(KIB, 10 + i));
    }
    common::plug_and_catalog(&mut store, &provider, &a);
    let stats = run_fingerprint(&mut store, &AtomicBool::new(true)).unwrap();
    assert_eq!(stats.size_candidates, 5);
    assert_eq!(stats.quick_hashed, 0);
}
