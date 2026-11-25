-- Digital Estate Management (Capability #15 / 5.3, reshaped R10, D1, D3, R2, R7, R8):
-- trustees, non-secret K-of-N Shamir shard metadata, sealed succession packets, and
-- non-destructive check-in history inside SQLCipher `aegis.db`.
CREATE TABLE IF NOT EXISTS estate_trustees (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    relationship TEXT NOT NULL DEFAULT '',
    contact_hint TEXT NOT NULL DEFAULT '',
    role         TEXT NOT NULL DEFAULT 'co_trustee',
    notes        TEXT NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS estate_shard_records (
    id              TEXT PRIMARY KEY,
    shard_set_id    TEXT NOT NULL,
    trustee_id      TEXT REFERENCES estate_trustees(id) ON DELETE SET NULL,
    share_index     INTEGER NOT NULL,
    threshold_k     INTEGER NOT NULL,
    total_n         INTEGER NOT NULL,
    key_fingerprint TEXT NOT NULL,
    checksum_prefix TEXT NOT NULL,
    delivered_at    INTEGER,
    verified_at     INTEGER,
    revoked_at      INTEGER,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_estate_shards_set ON estate_shard_records(shard_set_id, share_index);

CREATE TABLE IF NOT EXISTS estate_packets (
    id                        TEXT PRIMARY KEY,
    title                     TEXT NOT NULL,
    trustee_id                TEXT REFERENCES estate_trustees(id) ON DELETE SET NULL,
    packet_kind               TEXT NOT NULL DEFAULT 'letter',
    body_markdown             TEXT NOT NULL DEFAULT '',
    include_backup_map        INTEGER NOT NULL DEFAULT 1,
    include_project_inventory INTEGER NOT NULL DEFAULT 1,
    created_at                INTEGER NOT NULL,
    updated_at                INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS estate_checkins (
    id            TEXT PRIMARY KEY,
    method        TEXT NOT NULL DEFAULT 'manual',
    note          TEXT NOT NULL DEFAULT '',
    checked_in_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_estate_checkins_at ON estate_checkins(checked_in_at DESC);
