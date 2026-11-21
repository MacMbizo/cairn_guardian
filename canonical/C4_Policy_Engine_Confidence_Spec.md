# C4 — Policy Engine, Confidence Routing & Autonomy Specification (v1.1)

**ID:** C4 · **Version:** 1.1 · **Status:** Approved MVP Baseline
**Owner:** Staff Automation & Safety Architect · **Last Updated:** 2026-11-20
**Related Docs:** `A2_PRD_v2.1.md` (`v2.1.2`, `FR-005`), `B4_Data_Vector_Schema_v2.3.md` (`v2.3.1`), `B8_ADRs.md` (`v2.3`, `ADR-003`, `ADR-004`), `C1_IPC_Action_Contracts.md` (`v1.2`)

> **v1.1 changes vs v1.0:** Added `safe_build_cache` carve-out in Step 3 for registered `cache_dirs` (`node_modules`, `target/`, `.venv/`, `.gradle/`); marked `security_remediate` (`move_to_vault`/`encrypt`) as V2-reserved (disabled in MVP UI); fixed §4 Reference Pseudocode to check both `from_path` and `to_path` against `.aegis` internals, enforce `ActionType::Purge` explicit confirmation, and enforce `Housekeeper`/`Security` agent ceilings 1:1 with §3.

---

## 1. Purpose & Scope
Defines the deterministic evaluation rules governing how `ActionProposals` (from Agents, Rules, Dedupe, or User) pass through the **Policy Engine** before entering `approved` or `executed` states. Eliminates ambiguity between `autonomy_dial`, confidence thresholds, agent ceilings, and `atomic_units` guards.

**Audience:** Rust Automation Engine engineers, Agent Orchestrator engineers, Frontend Simulator devs, QA.

## 2. Autonomy Dial & Action Classes
Autonomy is configured **per action class** in `settings` (`scope = 'global' | 'root:<id>' | 'project:<id>'`), never as a single global override:

| Action Class | MVP (`v1.0`) Max Allowed Dial | V2+ Max Allowed Dial |
|---|---|---|
| `refile` (`move` / `rename` via Curator/Rules) | `Suggest` (Manual approval in Simulator/Inbox) | `Auto` (if `confidence ≥ 0.90` & user-enabled) |
| `archive` (`Warm → Cold` lifecycle) | `Manual` (tags only in MVP) | `Auto-approved` (V2) |
| `quarantine` (Dedupe / Housekeeper) | **`Suggest` (Never Auto)** | **`Suggest` (Never Auto)** |
| `purge` (Permanent delete) | **`Manual` (Explicit confirm only)** | **`Manual` (Explicit confirm only)** |
| `security_remediate` (`move_to_vault` / `encrypt`) | `V2 reserved (disabled in MVP UI)` | `Suggest` |

> **MVP Invariant (`A2` §7):** In `v1.0`, **zero** agent-generated proposals auto-execute without explicit user approval in Simulator or Approvals Inbox. Graduated deterministic rules (`source = 'graduated_proposal'`) may pre-stage approved drafts only if user explicitly sets that rule's action to `Auto-approved` in V1.5+.

## 3. Precedence & Deterministic Decision Pipeline
Every `actions` row is evaluated in strict order (`1 → 6`). **First failing hard gate blocks execution** (`policy_blocked:<reason>`):

1. **Root Boundary Check:** Canonicalized `from_path` and `to_path` (if non-null) must reside inside active, `online` configured roots in `volumes` (`..` traversal and escaping symlinks rejected).
2. **Quarantine / Internal Exclusion Check:** Neither `from_path` nor `to_path` (if present) may target `.aegis` internals (`aegis.db`, `models/`, logs), **except**: `ActionType::Quarantine` may set `to_path` inside `<VolumeRoot>/.aegis-quarantine`, and `ActionType::Restore` / `ActionType::Purge` may set `from_path` inside `<VolumeRoot>/.aegis-quarantine`.
3. **Atomic Unit Guard (`ADR-004`):** If the target belongs to an `atomic_unit` (`files.atomic_unit_id IS NOT NULL` or `cache_dirs.atomic_unit_id IS NOT NULL`) AND `to_path` crosses outside `atomic_units.root_path` (or quarantines/purges an interior path), **BLOCK** unless:
   - `atomic_units.allow_split_override == 1`, OR
   - `files.unit_split_override == 1` (for file targets), OR
   - **`safe_build_cache` carve-out:** `target_kind == 'dir'` matches a registered `cache_dirs` entry (`node_modules`, `target`, `.venv`, `.gradle`) AND `action_type == ActionType::Quarantine` (allowed in `Suggest` mode only — never `Auto`).
4. **Purge Gate, Agent Ceiling & Autonomy Check:**
   - `ActionType::Purge` always requires explicit permanent-delete confirmation (`RequireExplicitPermanentConfirm`).
   - Compare `actions.agent` against `settings['autonomy.<action_class>']`. Even in V2 `Auto` mode, `Housekeeper` and `Security` agents remain capped at `Suggest`.
5. **Confidence Routing Band:**
   - `confidence ≥ 0.90`: **Auto-Eligible** (routed to "High-Confidence Ready" group in Simulator; eligible for V2 `Auto`).
   - `0.60 ≤ confidence < 0.90`: **Review Queue** (highlighted amber in Simulator with evidence required).
   - `confidence < 0.60`: **Hold / Flag Only** (never proposed as an automatic move; surfaced only as "Uncertain — Manual Sort" suggestion).
   - Deterministic `rules` (`agent IS NULL`) carry `confidence = 1.0`.
6. **Rule Conflict & Priority Resolution:** When multiple `rules` match a file:
   - Lower `rules.priority` integer wins (e.g., `10` overrides `100`).
   - Ties broken by specificity: `project:<id>` > `root:<id>` > `global`.
   - Remaining ties mark `status = 'proposed'` with `conflict_note` for manual pick (never nondeterministic).

## 4. Reference Evaluation Pseudocode
```rust
fn evaluate_policy(action: &Action, ctx: &PolicyContext) -> PolicyVerdict {
    if !ctx.within_active_roots(&action.from_path, action.to_path.as_deref()) {
        return PolicyVerdict::Block("outside_configured_roots");
    }
    if ctx.is_protected_internal_for(action) {
        // Checks BOTH from_path and to_path against .aegis internals, exempting
        // Quarantine to_path and Restore/Purge from_path inside <VolumeRoot>/.aegis-quarantine
        return PolicyVerdict::Block("protected_aegis_internal");
    }
    if ctx.crosses_atomic_unit(action)
        && !ctx.split_override_enabled(action)
        && !ctx.is_safe_build_cache_quarantine(action)
    {
        return PolicyVerdict::Block("atomic_unit_boundary");
    }
    if action.action_type == ActionType::Purge {
        return PolicyVerdict::RequireExplicitPermanentConfirm;
    }
    if action.action_type == ActionType::Quarantine {
        return PolicyVerdict::RequireApproval("quarantine_never_auto");
    }
    if ctx.is_agent_capped_suggest(action.agent.as_deref()) {
        // Housekeeper and Security agents never exceed Suggest ceiling
        return PolicyVerdict::RequireApproval("agent_ceiling_suggest");
    }
    if action.confidence < 0.60 {
        return PolicyVerdict::HoldLowConfidence;
    }
    if ctx.is_mvp_mode() || ctx.dial_for(action) != Autonomy::Auto || action.confidence < 0.90 {
        return PolicyVerdict::RequireApproval("manual_or_review_gate");
    }
    PolicyVerdict::AutoEligible
}
```

## 5. Metadata, Open Questions, Assumptions & Traceability
- **Acceptance:** Unit & property tests cover 100% of truth-table combinations (`dial × confidence × atomic_unit × safe_build_cache × agent_ceiling × protected_internal`).
- **Open:** None for MVP.
- **Assumptions:** `rules.priority` defaults to `100`; built-in safety rules use priority `1–20`.
- **Traceability:** Upstream `A2` `FR-005.1`–`FR-005.3`, `B8` `ADR-004`, `B6` §3; Downstream `C1 v1.2` `actions.simulate/execute`, `D3 v1.1` Simulator UX.

## 6. Amendment 2026-11-21: Standing approval for backup top-ups
*Source: Drives & Duplicates design (`docs/superpowers/specs/2026-11-21-drives-and-duplicates-design.md` §7.8.6).*

This is the first and only exception to the v1.0 invariant that no proposal auto-executes.

- **Scope.** Only `backup_topup` runs on the drive holding the Backup role. A run may (a) copy files onto that drive, under `Aegis Backup\`, and (b) move or prune copies inside `Aegis Backup\` (previous versions, the 90-day prune). It never deletes, moves or renames anything at a source. Duplicate quarantines and copy-then-quarantine moves always go through normal approval.
- **Earning it.** It is granted by the owner (`grant_standing_topup`). A grant requires at least one completed top-up on that drive that the owner approved.
- **Binding.** The approval is tied to the drive's **passport id**, not its volume id or letter. A drive that no longer carries that passport does not match.
- **Revocation (audited).** Each of these revokes it:
  - clone detection involving the passport;
  - re-passporting the drive;
  - the drive leaving the Backup role;
  - a new grant, which supersedes the old one.
- **Guards still apply.** Every run re-checks that each planned path stays inside `Aegis Backup\` on that drive. The mass-change and encryption guard halts affected folders for an explicit owner answer.
- **Audit.** The grant, every revocation and a summary of every standing run are written to `audit_log` (`standing_approval.granted`, `standing_approval.revoked`, `backup.topup` with `authority: standing`).

## 7. Amendment 2026-11-21: Graduated filing rules
*Source: Auto-filing design (`docs/superpowers/specs/2026-11-21-auto-filing-design.md` §9). Owner decision: learn-then-take-over.*

This is the second exception to the v1.0 invariant that no proposal auto-executes. It follows the same standing-approval pattern as §6.

- **Scope.** Only `Move` and `Rename` actions proposed by the filing agent (`agent = 'filing'`) for one **filing rule**: a `(kind, category, who)` key with its destination template. Never `Quarantine`, `Purge` or `Archive`. Never anything inside an atomic unit. Never a move between drives. Never the Backup drive.
- **Earning it.** A rule becomes automatic after **3 owner approvals** of its moves, with no rejection or undo since. The engine may then approve and execute that rule's new moves **only when their confidence is ≥ 0.90**. Lower-confidence matches are still proposed for approval.
- **Where it applies.** The rule applies in every scope where the owner approved it, and on the PC. A rule learned on one drive only acts on another drive once the owner has approved it there.
- **Preconditions.** The scope has a chosen blueprint, and the owner has approved at least one filing batch there.
- **Revocation.** Any of these returns the rule to asking, with approvals reset to 0:
  - the owner rejects or unticks one of the rule's moves;
  - the owner undoes (rolls back) a batch that contained one;
  - the owner chooses "Stop automatic".
- **Audit and undo.** Automatic moves run as ordinary action batches (origin `filing-auto:<scope>`). They are journaled, listed in the weekly note, and each one can be undone like any other batch.
