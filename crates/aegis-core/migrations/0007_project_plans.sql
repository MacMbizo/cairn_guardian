-- Project plans (spec 2026-11-22-context-aware-filing-design §8; plan 2026-11-23-project-plans).
-- Steps inside one batch run in an explicit order, so a step that frees a name runs before the
-- move that needs it. Existing batches all get step 0 and keep their order.
ALTER TABLE actions ADD COLUMN step INTEGER NOT NULL DEFAULT 0;

-- Computed on each recognition pass from project_plan_runs (collection ids are not stable).
ALTER TABLE collections ADD COLUMN plan_state TEXT;
-- The owner said "keep this copy here": the plan gives it no step.
ALTER TABLE collection_locations ADD COLUMN keep_here INTEGER NOT NULL DEFAULT 0;

-- Every project plan that was proposed or run, keyed by the fingerprints of the locations it
-- touched, so a recognition pass can tell which project a run belongs to.
CREATE TABLE project_plan_runs (
    batch_id TEXT PRIMARY KEY,
    volume_id TEXT NOT NULL,
    fingerprints_json TEXT NOT NULL,
    state TEXT NOT NULL,
    at INTEGER NOT NULL
);
