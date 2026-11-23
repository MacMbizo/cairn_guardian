# Aegis Guardian — Design System Master Specification (`D1`)

**Version:** 1.0 · **Status:** Approved UI/UX Target (Scheduled for Unified Implementation Post-Feature Drop)  
**Generated via:** `ui-ux-pro-max` Design Intelligence · **Target Stack:** Tauri 2.x + React 19 + TypeScript + Tailwind CSS v4  
**Related Canonical Specs:** [`D1_Design_System_v1.0.md`](../../canonical/D1_Design_System_v1.0.md) · [`D3_Critical_UX_Flows_v1.0.md`](../../canonical/D3_Critical_UX_Flows_v1.0.md) · [`C4_Policy_Engine_Confidence_Spec.md`](../../canonical/C4_Policy_Engine_Confidence_Spec.md) · [`B6_Threat_Model.md`](../../canonical/B6_Threat_Model.md)

---

## 1. Visual Direction & Design Dials

| Dial / Dimension | Setting | Rationale |
|---|---|---|
| **Style** | **OLED Dark Mode / Flat Precision** | High legibility for long-session file triage; zero neon/3D glow clutter; crisp `1px` structural borders |
| **Variance** | `6 / 10` (Balanced / Modern) | Structured bento telemetry cards paired with split-pane diffs and command-first navigation |
| **Motion** | `4 / 10` (Subtle / Functional) | `150–200ms` ease transitions; zero gratuitous bounce; `prefers-reduced-motion` skips directly to final state |
| **Density** | `8 / 10` (Dense / Desktop Console) | Tight `8–32px` spacing scale maximizing scannable paths, confidence pills, and byte metrics |

---

## 2. Color Tokens (WCAG 2.2 AA `≥ 4.5:1` Contrast)

Define these semantic CSS custom properties in `src/index.css` (`@theme` / `:root`) and reference semantic tokens rather than raw hex values inside components:

```css
:root {
  color-scheme: dark;

  /* Surfaces */
  --color-background: #0F172A;      /* Primary app canvas */
  --color-card: #1B2336;            /* Elevated panels, modals, cards */
  --color-primary: #1E293B;         /* Sidebar, sticky headers, toolbars */
  --color-secondary: #334155;       /* Secondary controls & chips */
  --color-muted: #272F42;           /* Hover surfaces, inset tracks */

  /* Text */
  --color-foreground: #F8FAFC;      /* Primary readable copy (16:1+ on #0F172A) */
  --color-on-primary: #FFFFFF;
  --color-on-secondary: #FFFFFF;
  --color-muted-foreground: #94A3B8;/* Secondary paths, timestamps, counts */

  /* Borders & Focus */
  --color-border: #475569;          /* Prominent borders / dividers */
  --color-border-subtle: #272F42;   /* Card outlines */
  --color-ring: #FFFFFF;            /* High-visibility keyboard focus ring */

  /* Semantic Status & Actions */
  --color-accent: #22C55E;          /* Safe reversible CTA / Ready ≥90% / Healthy */
  --color-on-accent: #0F172A;       /* Dark ink on #22C55E (>= 7.5:1 contrast) */
  --color-warning: #F59E0B;         /* Staged Quarantine (30d) / Review 60-89% */
  --color-warning-strong: #D97706;  /* Amber borders / striped fill base */
  --color-destructive: #EF4444;     /* Permanent purge / Atomic Unit lock / Error */
  --color-on-destructive: #0F172A;
}
```

**Color-Alone Prohibition:** Every status indicator (`Ready`, `Review`, `Atomic Unit Locked`, `Staged`, `Healthy`, `Failed`) MUST pair hue with an explicit **Lucide SVG icon** and a **text label or pattern fill** (never color dots or colored badges without text/icon).

---

## 3. Typography & Zero-Egress Asset Rule

- **UI / Prose Font:** `IBM Plex Sans` (`400`, `500`, `600`, `700`) — crisp technical clarity, tabular numbers for storage counts.
- **Technical / Path Font:** `JetBrains Mono` (`400`, `500`, `600`) — file paths, `BLAKE3` fingerprints, byte sizes, confidence percentages, and keyboard shortcut badges (`Ctrl+K`, `Esc`).
- **Zero-Egress Invariant (`B6` §4):** Both font families **MUST be bundled locally as `.woff2` files** via Vite (e.g., `src/assets/fonts/` or `@fontsource/ibm-plex-sans` + `@fontsource/jetbrains-mono`). **Never** load Google Fonts or any external CSS/CDN at runtime; runtime packet capture in `F2` must observe `0` non-localhost requests.
- **Minimum Readability:** Body UI text `14px` (`0.875rem`), metadata/mono badges `≥ 12px` (`text-xs` minimum). Never truncate critical filename stems or destination conflict filenames without a hover/focus full-path tooltip.

---

## 4. Layout, Spacing, Motion & Performance Rules

- **Spacing Scale (`8–32px` dashboard density):**
  `--space-xs: 4px`, `--space-sm: 8px`, `--space-md: 12px`, `--space-lg: 16px`, `--space-xl: 24px`, `--space-2xl: 32px`.
- **Window & Responsive Breakpoints:** Minimum desktop window `960×640` (per `tauri.conf.json`), tested cleanly at `1024px`, `1280px`, and `1440px` without horizontal page overflow.
- **List Virtualization (`A2 §4.1` `≤350MB` WebView cap):** Any list exceeding `100` items (Search results, Duplicate clusters, Quarantine items, Simulator rows, Audit log) **MUST** be virtualized (`@tanstack/react-virtual`) rather than rendering thousands of DOM nodes.
- **Motion Discipline:** Use CSS transitions `150ms–200ms cubic-bezier(0.2, 0, 0, 1)` for hover, drawer slide-in, and focus rings; avoid animating `width`/`height` or layout shifts (`CLS < 0.1`). Honor `@media (prefers-reduced-motion: reduce)` unconditionally.

---

## 5. Data Visualization Standards

- **Prohibited:** Pie/donut charts for storage breakdown (`ui-ux-pro-max` risk:high for small slice discrimination).
- **Required:**
  1. **100% Stacked Segmented Horizontal Bar** for `Storage & Quarantine Clarity` — separates *Active Files*, *Staged in Quarantine (30d reversible)* with a **diagonal 45° SVG/CSS stripe pattern** + amber hue, and *Free Space* in emerald, paired with large explicit numeric readouts (`18.4 GB Staged` vs `4.2 GB Actually Freed on Disk`).
  2. **Segmented Health Bars + Clickable Offender Drilldowns (`FR-006`)** — 5 horizontal progress bars (`Duplication Hygiene`, `Build Caches`, `Secrets & PII`, `Index Coverage`, `Refiling Queue`) where every row opens its exact top-10 offending files/folders.
  3. **Collapsible Tree Table** for any hierarchical folder/treemap drilldowns in V1.5+ (`F18`).

---

## 6. Accessibility & Zero-Friction Interaction Contract (`FR-022`, WCAG 2.2 AA)

- **Focus Not Obscured:** Main scrollable containers set `scroll-padding-top` equal to the sticky header height so focused rows are never hidden beneath sticky bars.
- **Visible Focus Everywhere:** Every button, chip, link, row, and modal radio card preserves a `2px` `--color-ring` / `--color-accent` focus outline (`focus-visible`).
- **Global & Batch Keyboard Navigation:**
  - `Ctrl+K` / `Cmd+K` focuses the **Universal Command & Search Omnibar**.
  - `j` / `k` or `↑` / `↓` navigates rows in Search, Disk Rescue, and Simulator; `Space` toggles row selection; `→` opens preview; `Ctrl+Enter` approves/executes primary safe action; `Esc` dismisses overlays and returns focus to the trigger.
  - `aria-live="polite"` status region announces scan completion, batch execution, and rollback outcomes without disruptive OS alert spam.
- **Native Folder Picking & Drag-and-Drop:** Replace manual text-only path entry with `@tauri-apps/plugin-dialog` native directory picker + full-window folder drag-and-drop + auto-detected OS folder chips (`Documents`, `Downloads`, `Desktop`, `Projects`), retaining direct path paste as a fast power-user fallback.

---

## 7. Unified Modular Component Architecture (Target Refactor)

When implementing after feature completion, replace monolithic `src/App.tsx` views with these shared components:

| Component | Purpose | Page / Scope |
|---|---|---|
| `CommandOmnibar` | Top `Ctrl+K` semantic search + `type:` / `quarantined:` filter chips + `>` action commands + right preview drawer | Global Header |
| `AmbientTrustRail` | Compact sidebar footer (`Hardware Profile · LLM Idle 0 MB`, `Offline · 0 B Egress`, live `Doctor: 8/8` dot) | Global Sidebar |
| `StorageClarityBar` | 100% stacked striped bar + Staged (30d) vs. Actually Freed split metrics + 10% quota alert | Dashboard, Disk Rescue |
| `CoverageRemediationDrawer` | Honest coverage banner (`never 100% if skipped > 0`) + slide-over drawer grouping permission/cloud/locked paths | Dashboard |
| `SimulatorSplitDiff` | Auto-simulated Before → After split-pane tree diff, `?` 3-signal popover, `Auto-rename` chips, Atomic Unit locks & escape hatches, `"Teach Aegis Once"` toast | Simulator & Approvals |
| `RollbackConflictModal` | Batch pre-flight reassurance banner + keyboard-navigable radio option cards + non-destructive defaults | Global Rollback Flow |

---

## 8. Page Specs & Visual Mockups

- **Dashboard & Command Hub:** [`pages/dashboard.md`](pages/dashboard.md) · [`assets/dashboard-command-hub.jpg`](assets/dashboard-command-hub.jpg)
- **Split-Pane Reorganisation Simulator:** [`pages/simulator.md`](pages/simulator.md) · [`assets/simulator-split-diff.jpg`](assets/simulator-split-diff.jpg)
- **Pre-Rollback 3-Way Conflict Resolver:** [`pages/rollback-conflict.md`](pages/rollback-conflict.md) · [`assets/rollback-conflict-resolver.jpg`](assets/rollback-conflict-resolver.jpg)

---

## 9. Pre-Implementation Delivery Checklist

- [ ] `IBM Plex Sans` & `JetBrains Mono` `.woff2` files bundled locally (`0` external requests verified in `F2`).
- [ ] All interactive controls use `<button>`/`<a>`/`<input>` with `cursor-pointer` and visible `:focus-visible` ring (`≥ 4.5:1` contrast).
- [ ] Zero emoji icons anywhere; Lucide SVG icons paired with text/pattern labels on every status pill.
- [ ] Lists `> 100` items virtualized (`@tanstack/react-virtual`).
- [ ] Honest Coverage never renders `100%` when `files_skipped_perm > 0` or `files_skipped_cloud > 0`.
- [ ] All destructive actions (`quarantine.purge(..., confirmedPermanent=true)`, `Abort Rollback`) require explicit confirmation and clear destructive styling.
