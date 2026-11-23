# D1 — Aegis Guardian Design System & Zero-Friction UX Target (v1.0)

**ID:** D1 · **Version:** 1.0 · **Status:** Approved UI/UX Target (Scheduled for Unified Implementation Post-Feature Drop)  
**Owner:** Staff Product Designer · **Last Updated:** 2026-11-23  
**Related Docs:** `D3_Critical_UX_Flows_v1.0.md` (`v1.1`), `A2_PRD_v2.1.md` (`v2.1.2`), `C1_IPC_Action_Contracts.md` (`v1.2`), `C4_Policy_Engine_Confidence_Spec.md` (`v1.1`), `B6_Threat_Model.md` (`v2.4`), `V1.5_Feature_Backlog.md` (`v1.1`)  
**Implementation Reference Assets:** [`../design-system/aegis-guardian/MASTER.md`](../design-system/aegis-guardian/MASTER.md)

---

## 1. Purpose & Scope

Establishes the authoritative visual design system, component architecture, and zero-friction UX upgrades for Aegis Guardian (`OQ-16` resolution), replacing the Sprint 0 engineering console in `src/App.tsx` once planned core backend features are complete so the entire interface can be implemented cleanly in one unified pass.

## 2. Locked Visual & Technical Decisions

1. **Aesthetic Direction:** **OLED Dark Mode / Flat Precision** (`ui-ux-pro-max` dials: Variance `6/10`, Motion `4/10`, Density `8/10`) — `#0F172A` canvas, `#1B2336` card surface, `1px` `#272F42`/`#475569` borders, `#22C55E` safe reversible CTA (`#0F172A` ink), `#F59E0B` quarantine/review, `#EF4444` destructive/atomic-unit lock. Zero neon glow or 3D decoration.
2. **Typography & Zero-Egress Invariant (`B6` §4):** `IBM Plex Sans` (UI) + `JetBrains Mono` (paths, `BLAKE3` hashes, bytes, shortcuts) bundled **strictly as local Vite `.woff2` assets** — never external font CDNs at runtime.
3. **Accessibility & Performance (`FR-022`, `A2 §4.1`):**
   - WCAG 2.2 AA `≥ 4.5:1` contrast, `scroll-padding-top` so sticky headers never obscure `:focus`, visible focus rings across modals, color always paired with Lucide SVG icon + text/pattern label, and `prefers-reduced-motion` support.
   - List virtualization (`@tanstack/react-virtual`) mandatory for lists `> 100` rows to hold WebView RSS `≤ 350MB`.
4. **Data Visualization Standard:** Replace color-only/pie charts with **100% stacked segmented horizontal bars** (with diagonal stripe fill on staged quarantine) and clickable **5-dimension Health bars** (`FR-006`).

## 3. Six Zero-Friction UX Upgrades Over Sprint 0

| # | Upgrade | Key Improvement | Page Spec & Mockup |
|---|---|---|---|
| 1 | **Command Hub Omnibar (`Ctrl+K`)** | Combines semantic search, filter chips (`type:`, `quarantined:off`), `>` commands, inline quarantine-match nudge, and live preview drawer in one sticky header | [`dashboard.md`](../design-system/aegis-guardian/pages/dashboard.md) · [`dashboard-command-hub.jpg`](../design-system/aegis-guardian/assets/dashboard-command-hub.jpg) |
| 2 | **Zero-Typing Root Onboarding** | Auto-detected OS user folder chips (`Documents`, `Downloads`, `Desktop`, `Projects`) + native `@tauri-apps/plugin-dialog` picker + drag-and-drop folder addition | [`dashboard.md`](../design-system/aegis-guardian/pages/dashboard.md) |
| 3 | **Coverage Remediation Drawer & Storage Clarity Bar** | Honest Coverage banner (`Review & Fix`) opens grouped remediation slide-over; 100% stacked bar visually separates `Staged in Quarantine (30d)` vs. `Actually Freed on Disk` (`V1.5-01`) | [`dashboard.md`](../design-system/aegis-guardian/pages/dashboard.md) |
| 4 | **Ambient Trust Rail** | Surfaces `Hardware Profile · LLM Idle 0 MB`, `Offline · 0 B Egress`, and live `Doctor: 8/8 Healthy` indicator in sidebar footer | [`dashboard.md`](../design-system/aegis-guardian/pages/dashboard.md) |
| 5 | **Auto-Simulated Split Diff + "Teach Aegis Once"** | Compresses 5-click `Propose → Simulate → Approve → Execute` ceremony into 1 explicit `Approve & Apply` action over Before/After split diff with `?` 3-signal popover, `Auto-rename` preview, Atomic Unit `[Move Whole Repo]`/`[Unlock File]` escape hatches, and post-apply `Create Rule` / `Undo` toast | [`simulator.md`](../design-system/aegis-guardian/pages/simulator.md) · [`simulator-split-diff.jpg`](../design-system/aegis-guardian/assets/simulator-split-diff.jpg) |
| 6 | **Calm Pre-Rollback 3-Way Conflict Resolver** | Replaces bare `<select>` dropdowns with pre-flight `"0 files mutated yet"` banner and keyboard-navigable radio cards pre-selecting non-destructive safe defaults | [`rollback-conflict.md`](../design-system/aegis-guardian/pages/rollback-conflict.md) · [`rollback-conflict-resolver.jpg`](../design-system/aegis-guardian/assets/rollback-conflict-resolver.jpg) |

## 4. Metadata & Traceability
- **Acceptance:** Unified frontend refactor implements all 6 component groups (`CommandOmnibar`, `AmbientTrustRail`, `StorageClarityBar`, `CoverageRemediationDrawer`, `SimulatorSplitDiff`, `RollbackConflictModal`), passes `F2` `0` external packet check with bundled fonts, and passes `D3` §7 keyboard-only Playwright checks.
- **Deferred Optional Item:** Light-mode token override sheet (`OQ-16` dark baseline is complete; light mode can be layered later via semantic tokens if requested).
- **Traceability:** Upstream `A2 v2.1.2`, `D3 v1.1`, `C1 v1.2`, `C4 v1.1`; Implementation blueprint [`design-system/aegis-guardian/MASTER.md`](../design-system/aegis-guardian/MASTER.md).
