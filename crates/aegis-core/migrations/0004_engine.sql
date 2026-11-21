-- Always-on engine (spec 2026-11-21-always-on-engine-design): watch bookkeeping, reminders,
-- space history and weekly notes.
CREATE TABLE watch_state (
    name TEXT PRIMARY KEY,
    last_run_at INTEGER,
    last_ok_at INTEGER,
    last_error TEXT
);

CREATE TABLE reminders (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    subject TEXT NOT NULL,
    level TEXT NOT NULL,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    bytes INTEGER,
    action_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open',
    first_raised_at INTEGER NOT NULL,
    last_raised_at INTEGER NOT NULL,
    last_shown_at INTEGER,
    resolved_at INTEGER,
    UNIQUE (kind, subject)
);

CREATE TABLE reminder_kinds (
    kind TEXT PRIMARY KEY,
    dismiss_streak INTEGER NOT NULL DEFAULT 0,
    demoted INTEGER NOT NULL DEFAULT 0,
    demoted_at INTEGER,
    announced INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE space_samples (
    volume_id TEXT NOT NULL,
    day TEXT NOT NULL,
    used_bytes INTEGER NOT NULL,
    total_bytes INTEGER NOT NULL,
    PRIMARY KEY (volume_id, day)
);

CREATE TABLE root_samples (
    root TEXT NOT NULL,
    day TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (root, day)
);

CREATE TABLE weekly_notes (
    id TEXT PRIMARY KEY,
    week TEXT UNIQUE NOT NULL,
    built_at INTEGER NOT NULL,
    json TEXT NOT NULL,
    read_at INTEGER
);
