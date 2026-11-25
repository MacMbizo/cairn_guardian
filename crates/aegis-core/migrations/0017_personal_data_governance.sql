-- Capability #13: Personal Data Governance (`5.1`, reshaped `R7`-`R9`)
-- Stores reversible R7 sensitivity label batches (`K1`), pattern-based privacy rules,
-- and incremental PII/secret scan watermarks inside SQLCipher-encrypted `aegis.db` (`D1`).

CREATE TABLE governance_label_batches (
  id TEXT PRIMARY KEY,
  target_sensitivity TEXT NOT NULL,           -- public | internal | confidential | secret
  file_count INTEGER NOT NULL,
  reason TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'applied',     -- applied | undone
  created_at INTEGER NOT NULL,
  undone_at INTEGER
);

CREATE TABLE governance_label_entries (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL REFERENCES governance_label_batches(id) ON DELETE CASCADE,
  file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  prev_sensitivity TEXT NOT NULL,
  new_sensitivity TEXT NOT NULL,
  finding_id TEXT REFERENCES security_findings(id) ON DELETE SET NULL,
  prev_finding_status TEXT
);
CREATE INDEX idx_gov_label_entries_batch ON governance_label_entries(batch_id);
CREATE INDEX idx_gov_label_entries_file ON governance_label_entries(file_id);

CREATE TABLE governance_privacy_rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  path_prefix TEXT,
  filename_pattern TEXT,
  rule_action TEXT NOT NULL,                  -- auto_confidential | auto_secret | exclude_from_ai
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE governance_file_scans (
  file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
  content_mtime INTEGER NOT NULL,
  content_size INTEGER NOT NULL,
  findings_count INTEGER NOT NULL DEFAULT 0,
  scanned_at INTEGER NOT NULL
);
