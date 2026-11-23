# Page Spec: Split-Pane Reorganisation Simulator & "Teach Aegis Once"

**Reference Mockup:** [`../assets/simulator-split-diff.jpg`](../assets/simulator-split-diff.jpg)  
**Parent Design System:** [`../MASTER.md`](../MASTER.md)  
**Upstream Requirements:** `A2` `FR-005.1`–`FR-005.3`, `C4 v1.1`, `D3` §3, `B8 ADR-004`

---

## 1. Interaction Compression (Zero-Friction Safety)

- **Auto-Simulate on Propose:** Any action batch triggered from Disk Rescue, Dedupe, or Curator automatically runs `actions.propose` + `actions.simulate` and opens directly in the **Split-Pane Dry-Run Diff**—eliminating the current multi-tab `Propose → Simulate → Approve → Execute` ceremony.
- **Single Explicit Safe Confirmation:** Because `C4` MVP mode enforces `Suggest` (zero silent auto-execution), the primary top-right emerald CTA **`Approve & Apply N Safe Changes (Reversible 30d)`** atomically executes `actions.approve` + `actions.execute` only on checked, policy-allowed items.

---

## 2. Header & Confidence Triage Summary

- Title: `Simulator Dry-Run • Batch #<short_id>`
- Summary pills (clickable to filter rows):
  - **Green:** `N High-Confidence Ready` (`≥ 0.90`)
  - **Amber:** `N Review Needed` (`0.60 – 0.89`)
  - **Red Lock:** `N Atomic Unit Protected` (or blocked by `C4` policy)
- Grey `< 0.60` items (`HoldLowConfidence`) appear in a collapsed `"Uncertain — Manual Sort"` group, unchecked and non-executable by default.

---

## 3. Split-Pane Before → After Diff Table (`SimulatorSplitDiff`)

Virtualized two-column aligned rows with connecting `→` arrow indicators:

1. **Left Pane (`CURRENT LOCATIONS`):**
   - Selection checkbox + file/folder icon + monospace filename/relative path.
   - Confidence badge (`96%` emerald, `78%` amber) and policy context tags (e.g., `safe_build_cache` under `web-app/node_modules/`, or red lock `Atomic Unit Protected` on `rust-engine/Cargo.toml`).
2. **Right Pane (`PROPOSED DESTINATIONS`):**
   - Proposed path or `<VolumeRoot>/.aegis-quarantine/<batch_id>/... (O(1) rename, X GB staged)`.
   - **Collision Preview Badge:** If a destination file already exists, explicitly display blue-slate pill `Auto-rename → Q3_Brokerage_2026 (1).pdf` (never overwrite).
   - **Inline `"Why?"` Popover (`?` button):** Opens an anchored 3-signal evidence popover directly next to the row:
     1. Matched rule or classifier
     2. Nearest neighbor files in destination folder
     3. Extracted entities / metadata signals
   - **Atomic Unit Escape Hatches (`ADR-004`):** Blocked interior rows show a calm red-tinted inline banner (`Blocked: Inside Atomic Project Unit (Cargo.toml)`) with two direct action buttons—**`[Move Whole Repo]`** (`target_kind = 'atomic_unit'`) and **`[Unlock File]`** (`atomic_unit.override`)—instead of opaque error codes.

---

## 4. Post-Execution Feedback & Rule Graduation (`FR-005.3`)

- Immediately after applying a batch, show a non-blocking floating bottom toast:
  - Status checkmark: `"Applied N changes to <Folder>"`
  - Primary secondary CTA (emerald outline): **`Always apply this pattern → Create Rule`** (opens pre-filled `Rule` condition preview modal with live match count before saving via `rules.graduate` / `rules.create`).
  - Immediate **`Undo`** button triggering `actions.rollback(batch_id)` (entering the Pre-Rollback Conflict Resolver if external changes occurred).
