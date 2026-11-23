# D3 — Critical UX Flows & Accessibility Baseline (v1.1)

**ID:** D3 · **Version:** 1.1 · **Status:** MVP Design Baseline
**Owner:** Staff Product Designer · **Last Updated:** 2026-11-23
**Related Docs:** `D1_Design_System_v1.0.md` (`v1.0`), `A2_PRD_v2.1.md` (`v2.1.2`), `C1_IPC_Action_Contracts.md` (`v1.2`), `C4_Policy_Engine_Confidence_Spec.md` (`v1.1`)

> **v1.1 changes:** Aligned §4 Pre-Rollback conflict options with `C1 v1.2` (`modified` vs `collision`, removing fictional content backups) and added promoted `V1.5-01` Quarantine Space vs. Freed Space UI + 10% soft quota banner to §5 Disk Rescue.

---

## 1. Purpose & Scope
Specifies the interaction flows, edge-case states, honest-status copy, and WCAG 2.2 AA accessibility baseline (`FR-022`) for the 4 highest-risk MVP UX surfaces. Excludes visual brand tokens (color hexes/typography scale deferred to `D1` Design System).

**Audience:** Frontend/React engineers, Product Designers, QA.

## 2. Flow 1 — Onboarding, Hardware Profile & macOS TCC Recovery (`FR-011.1`)
```
[Welcome] ──▶ [Select Roots + Ignore Templates (node_modules, AppData)]
                    │
         (macOS only: Check Full Disk Access / Folder TCC)
           ├── Granted ──▶ [Detect Hardware Profile: Eco / Standard / Pro]
           └── Denied  ──▶ [Guided FDA Modal + "Continue with Limited Roots"]
                                │ (Logs skipped roots honestly in Coverage Report)
                                ▼
                    [Start Tier-1 Scan (≤2 min) → Unlock Health Dashboard]
```
- **Honest Coverage Rule:** Never show a "100% Complete" badge if `files_skipped_perm > 0` or `files_skipped_cloud > 0`. Display `"Indexed 94.2% (8,120 permission-skipped · 1,400 iCloud placeholders)"` with a clickable link to Remediation.

## 3. Flow 2 — Reorganisation Simulator & "Teach Aegis Once" (`FR-005.1`, `FR-005.3`)
1. **Diff View:** Left pane shows current folder tree; right pane shows proposed tree. Every row displays: `confidence` pill (`≥0.90` Green, `0.60–0.89` Amber, `<0.60` Grey-Locked), `"Why?"` popover (matched rule/project + top 3 evidence signals), and Atomic Unit lock icon if protected (`ADR-004`).
2. **Collision Preview:** Colliding destinations explicitly display `"Auto-rename → report (1).pdf"` (never overwrite).
3. **Approve & Graduate (`FR-005.3`):** Clicking **Approve** on a proposal triggers a non-blocking toast/inline option: *"Always apply this pattern? [Create Rule]"* → opens pre-filled `Rule` preview modal showing exact `condition_json` + matched preview count before saving via `rules.create` / `rules.graduate`.

## 4. Flow 3 — Pre-Rollback 3-Way Conflict Dialog (`FR-005.4`, `C1 §4`)
Triggered when `actions.rollback(batch)` pre-flight checks return one or more items with `rollback_conflict != NULL` (prior to mutating any file in the batch):
- **Modal Header:** `"Cannot cleanly revert N of M files — external changes detected"`
- **Per-Conflict Row:** Shows `from_path`, `to_path`, conflict type (`Modified after Aegis moved it` | `Original location occupied` | `Moved file missing`), and explicit radio actions matching `C1 v1.2`:
  - `modified`: `Move edited file back to original path (revert_keeping_edits)` | `Copy edited file back & keep both (copy_back_keeping_both)` | `Skip this file` | `Abort entire rollback`
  - `collision`: `Restore as renamed copy (restore_as_rename)` | `Skip this file` | `Abort entire rollback`
  - `missing_target`: `Skip (mark unrecoverable_missing)` | `Abort entire rollback`
- **Safety Default:** Destructive-avoidance default = `copy_back_keeping_both` for `modified`; `restore_as_rename` for `collision`; `Skip` (`unrecoverable_missing`) for `missing_target`.

## 5. Flow 4 — Crisis Playbooks (`FR-011.2`)
- **Disk Rescue:** Accessed from Dashboard card `"Reclaimable Storage"`. Header splits **`Staged in Quarantine (Reversible, 30d)`** vs. **`Actually Freed on Disk`** (`quarantine.space_summary`) with a soft-quota warning if quarantine exceeds 10% of volume size or free space drops `<10%`, plus an explicit **"Empty Quarantine Now"** confirmation action (`quarantine.purge(..., confirmedPermanent=true)`). Groups into 3 expandable tabs: *Exact Duplicates (hardlink-safe)*, *Build Caches (`node_modules`, `target/` from `cache_dirs`)*, *Stale Oversized Files (>500MB, >180d untouched)*. Primary CTA: `"Move Selected to Volume Quarantine"` (never silent permanent delete).
- **Pre-Share Secret Audit:** Folder picker → runs `findings.scan(path)` → renders deduplicated secrets sorted by `context_risk_score` with masked snippets (`AKIA...9X2Q`) and 1-click verdicts (`false_positive`, `accepted_risk`, `resolved`; `vaulted` disabled in MVP UI).

## 6. Accessibility & Keyboard Baseline (WCAG 2.2 AA, `FR-022`)
- Global Search hotkey (`Cmd/Ctrl + Shift + Space` or configurable) + `Cmd/Ctrl + K` command palette.
- Full keyboard navigation (`Tab`, `↑/↓`, `Enter` open, `Space` select, `Escape` close modal) across Search, Simulator Diff, and Approvals Inbox.
- All status badges pair color with icon + text label (never color-only); `aria-live="polite"` regions announce scan tier completion and rollback results.

## 7. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** Interactive Storybook/Playwright tests verify keyboard-only completion of Flows 1–4 and zero "100%" fake badges when skipped counts > 0.
- **Open / Resolved:** `[RESOLVED — D1 v1.0 locked]` `OQ-16` (Owner: Design Lead, 2026-11-23): OLED Dark Mode token palette, local typography, and WCAG 2.2 AA contrast rules finalized in `D1_Design_System_v1.0.md` and `design-system/aegis-guardian/MASTER.md` (optional light-mode sheet deferred).
- **Traceability:** Upstream `A2` (`v2.1.2`, `FR-005`, `FR-011`, `FR-012`, `FR-022`), `D1` (`v1.0`), `C1 v1.2`, `C4 v1.1`; Downstream React/Tailwind components.
