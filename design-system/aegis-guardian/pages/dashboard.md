# Page Spec: Command Hub Dashboard (`Home & Health`)

**Reference Mockup:** [`../assets/dashboard-command-hub.jpg`](../assets/dashboard-command-hub.jpg)  
**Parent Design System:** [`../MASTER.md`](../MASTER.md)  
**Upstream Requirements:** `A2` `FR-001.1`, `FR-006`, `FR-007`, `FR-011.1`, `FR-011.2`, `FR-012/013`, `D3` §2 & §5

---

## 1. Layout Hierarchy

1. **Left Navigation Sidebar (`220px` fixed):**
   - Brand header: Aegis Guardian shield mark + wordmark.
   - Primary links (`≤ 5` top-level items): `Home & Health`, `Disk Rescue`, `Simulator (N)` (live pending count badge), `Security Inbox`, `Quarantine`.
   - **Ambient Trust Rail (Sidebar Footer):** Three compact, bordered status pills replacing hidden diagnostics tabs:
     - Hardware & LLM residency: `Standard Profile · LLM Idle 0 MB`
     - Egress guarantee: `Offline · 0 B Egress`
     - Live self-check dot (clickable shortcut to Doctor modal/view): `Doctor: 8/8 Healthy`
2. **Sticky Top Command Bar (`CommandOmnibar`):**
   - Centered `Ctrl+K` omnibar supporting natural-language search (`"tax pdfs 2025"`), inline syntax/filter chips (`type:pdf`, `quarantined:off`), and instant command triggers (`> scan`, `> rescue`, `> undo last`, `> doctor`).
   - Inline empty-hit nudge (bridge to `V1.5-02`): if active hits = `0` while `quarantined_files > 0`, show a non-intrusive chip right beneath the bar (`"Include quarantined files?" [Include]`).
   - Primary right-aligned CTA: **`Scan Roots`** (`#22C55E` emerald button with `#0F172A` text).

---

## 2. Core Dashboard Modules

### A. Zero-Typing Onboarding & Root Management (`FR-001.1`, `FR-011.1`)
- When no roots are configured (or via **Manage Roots** popover), offer:
  1. Auto-detected OS user folder chips (`Documents`, `Downloads`, `Desktop`, `Projects`) as 1-click toggles.
  2. Native OS directory picker (`@tauri-apps/plugin-dialog`).
  3. Full-window drag-and-drop folder dropzone.
  4. Power-user path paste fallback.
- Skippable onboarding (`User Freedom` guideline — never block the dashboard with a forced unskippable modal).

### B. Honest Coverage Banner & Remediation Drawer (`D3` §2, `G6`)
- Full-width amber-tinted banner when `files_skipped_perm > 0 || files_skipped_cloud > 0`:
  `"Indexed 96.4% of 142,810 files • 14 permission-locked • 38 cloud placeholders (never hydrated)"`
- Never renders `100% Complete` if any skipped count is `> 0`.
- **`Review & Fix`** opens `CoverageRemediationDrawer`, grouping errors by cause (*Permission-Locked*, *Cloud Placeholders*, *Locked by App*) with 1-click fixes (`Retry Locked`, `Add to .aegisignore`, `macOS FDA Guide`).

### C. Storage & Quarantine Clarity Card (`V1.5-01` promoted to v1.0)
- **100% Stacked Segmented Bar (`StorageClarityBar`):**
  - Slate segment: *Active Files*
  - Diagonally striped amber segment: *Staged in Quarantine 18.4 GB (30d reversible)*
  - Emerald segment: *Free Space*
- **Split KPI Readout:** Large amber **`18.4 GB`** (`Staged (Reversible 30d)`) side-by-side with emerald **`4.2 GB`** (`Actually Freed on Disk`).
- Direct secondary button **`Free 18.4 GB Now`** opens the explicit confirmation modal before invoking `quarantine.purge(..., confirmedPermanent=true)`. Displays an explicit quota alert badge when `quota_warning == true` (`≥ 10%` of volume).

### D. 2-Column Bento Row
- **Left — Environment Health (`88 / 100`, `FR-006`):** 5 horizontal progress bars (`Duplication Hygiene`, `Build Caches`, `Secrets & PII`, `Index Coverage`, `Refiling Queue`). Each row displays score + clickable offender count (`19 clusters >`, `4 repos >`) routing straight to the filtered fix view.
- **Right — Zero-Friction Quick Wins (`FR-011.2`):** High-leverage 1-click cards surfaced from existing queries:
  - Dormant build caches (`node_modules` / `target/`) with **`Stage Quarantine`** button (auto-simulates in Simulator).
  - Hardlink-safe exact duplicates with **`Review Keepers`** button.
