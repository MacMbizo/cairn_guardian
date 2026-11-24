-- Migration 0013: Autonomous Research Librarian (2.3) pinned reading-list links & dismissed pairs (D1, R5).

CREATE TABLE IF NOT EXISTS librarian_pins (
    source_file_id  TEXT NOT NULL,
    related_file_id TEXT NOT NULL,
    note            TEXT NOT NULL DEFAULT '',
    pinned_at       INTEGER NOT NULL,
    PRIMARY KEY (source_file_id, related_file_id)
);
CREATE INDEX IF NOT EXISTS idx_librarian_pins_source
    ON librarian_pins(source_file_id, pinned_at DESC);

CREATE TABLE IF NOT EXISTS librarian_dismissals (
    source_file_id  TEXT NOT NULL,
    related_file_id TEXT NOT NULL,
    dismissed_at    INTEGER NOT NULL,
    PRIMARY KEY (source_file_id, related_file_id)
);
CREATE INDEX IF NOT EXISTS idx_librarian_dismissals_source
    ON librarian_dismissals(source_file_id);
