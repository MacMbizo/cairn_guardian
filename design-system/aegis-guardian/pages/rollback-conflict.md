# Page Spec: Pre-Rollback 3-Way Conflict Resolver (`FR-005.4`)

**Reference Mockup:** [`../assets/rollback-conflict-resolver.jpg`](../assets/rollback-conflict-resolver.jpg)  
**Parent Design System:** [`../MASTER.md`](../MASTER.md)  
**Upstream Requirements:** `A2` `FR-005.4`, `C1 v1.2` §4, `D3` §4

---

## 1. Modal Purpose & Emotional Design

When users click **Undo / Rollback** and external edits, collisions, or deletions have happened after Aegis moved a file, raw `<select>` dropdowns create anxiety and confusion. This modal communicates **calm safety and zero hidden mutation**.

---

## 2. Layout & Copy Specification

1. **Modal Header:**
   - Amber shield-alert icon + `"Cannot cleanly revert N of M files — external changes detected"`
   - **Pre-Flight Reassurance Banner (Emerald-tinted):**
     Shield-check icon + `"Pre-flight safety pause: 0 files have been mutated on disk yet. Choose how to handle each conflict below."` (Directly reflects the `C1 v1.2` §4 batch pre-flight invariant.)
2. **Scrollable Conflict Cards (`RollbackConflictModal`):**
   Each card represents one `RollbackConflictItem` with:
   - Monospace path + semantic amber conflict pill:
     - `modified`: `"Modified after Aegis moved it"` + metadata delta (`BLAKE3 mismatch • +14.2 KB edited 12m ago • Original: <from_path>`)
     - `collision`: `"Original location reoccupied (Collision)"` + `"A newer file already exists at <from_path>"`
     - `missing_target`: `"Moved file missing on disk"`
   - **Selectable Radio Option Cards (replaces bare `<select>`):**
     Full-width keyboard-navigable radio cards (`↑`/`↓`, visible `--color-ring` focus, emerald active border, and explicit `"Recommended Safe Default"` badge on the pre-selected non-destructive option):
     - For **`modified`**:
       1. **[Recommended Default]** `Copy edited file back & keep both` (`copy_back_keeping_both`) — *"Preserves your edits in both locations; zero data loss."*
       2. `Move edited file back to original path` (`revert_keeping_edits`) — *"Reverts location while keeping your edits."*
       3. `Skip this file` (`skip`) — *"Leave at current destination."*
     - For **`collision`**:
       1. **[Recommended Default]** `Restore as renamed copy` (`restore_as_rename`) — *"Restores safely to `<name>.restored-<ts>.<ext>` (never overwrites)."*
       2. `Skip this file` (`skip`)
     - For **`missing_target`**:
       1. **[Recommended Default]** `Skip (mark unrecoverable_missing)` (`skip`)
       2. `Abort entire rollback` (`abort_batch`)
3. **Modal Footer:**
   - Left secondary danger-outline button: **`Abort Entire Rollback (Esc)`** (`abort_batch` — leaves all files untouched).
   - Right primary emerald button: **`Apply Safe Resolutions & Revert K Clean Files`**.
