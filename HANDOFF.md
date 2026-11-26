# Handoff: Cairn Guardian

Updated 2026-11-25 (**Capability #15: Digital Estate Management (5.3, reshaped R10)** — pure-Rust GF(256) $K$-of-$N$ Shamir secret splitting over `DbKey` with `blake3` checksums & non-secret DB metadata, `D3` trustee directory, `R2`/`R7` CredUI-gated sealed succession packets & offline Continuity Kit export, strictly non-destructive `R10` inactivity heartbeat, `estate_*` IPC, and `DigitalEstate` UI — built, verified, and fast-forward merged into `master`; all 15 North-Star roadmap capabilities are now complete). Read this first in a new chat.

**The whole-project view** (what is done, what is waiting on the owner, everything that remains) is in
`docs/PROGRESS.md`. Keep it current at the end of each plan.

Current state:
- `master`: Projects 1–3, Context-aware filing Plans 1–5, **Plan 6A** (SQLCipher whole-store encryption, `sqlite-vec` 0.1.9, DPAPI `R2`, 24-word/64-hex recovery, `R7`, `R9`), **Plan 6B** (local pure-Rust `aegis-embed`, encrypted `vec_chunks`, hybrid FTS5+vector RRF search, context filtering/enrichment, AC-gated `EmbedWatch`), **Project 4** (System Care: cleanup/dead files under `system_care` quarantine policy guard, `R3` read-only Uninstall/Startup + Windows settings links + leftover folder quarantine, non-admin CIM `SmartHealthWatch`, 30d/90d volume & root growth trends, upgraded 5-dimension `health_breakdown`), **Capability #3 (Local RAG Engine)** (`aegis-llm`, `core::rag`, `rag_*` IPC, `Ask Files`), **Capability #6 (Forgotten Asset Recovery)** (`migrations/0010_forgotten_assets.sql`, `core::assets`, `assets_*` IPC, `Forgotten Assets`), **Capability #7 (Semantic Version Tracking)** (`migrations/0011_version_tracking.sql`, `core::versions`, `versions_*` IPC, `Version Tracking`), **Capability #8 (Automatic Knowledge Extraction)** (`migrations/0012_knowledge_extraction.sql`, `core::knowledge`, `knowledge_*` IPC, `Knowledge Base`), **Capability #9 (Autonomous Research Librarian)** (`migrations/0013_research_librarian.sql`, `core::librarian`, `librarian_*` IPC, `Research Librarian`), **Capability #10 (Cross-Project Intelligence)** (`migrations/0014_cross_project.sql`, `core::cross_project`, `cross_project_*` IPC, `Cross-Project`), **Capability #11 (AI Folderless Mode)** (`migrations/0015_folderless_intents.sql`, `core::folderless`, `folderless_*` IPC, `Folderless Rules`), **Capability #12 (Self-Healing Workspace)** (`migrations/0016_self_healing.sql`, `ActionType::PatchText`, `core::healing`, `healing_*` IPC, `Self-Healing`), **Capability #13 (Personal Data Governance)** (`migrations/0017_personal_data_governance.sql`, `core::governance`, `governance_*` IPC, `Data Governance`), **Capability #14 (AI Chief Digital Officer Agent)** (`migrations/0018_cdo_agent.sql`, `core::cdo`, `cdo_*` IPC, `Chief Digital Officer`), and **Capability #15 (Digital Estate Management)** (`migrations/0019_digital_estate.sql`, `core::estate`, `estate_*` IPC, `Digital Estate`), fast-forward merged from `feature/digital-estate` (not pushed).
- `feature/digital-estate`, `feature/cdo-agent`, `feature/personal-data-governance`, `feature/self-healing-workspace`, `feature/folderless-mode`, `feature/cross-project-intelligence`, `feature/research-librarian`, `feature/knowledge-extraction`, `feature/semantic-version-tracking`, `feature/forgotten-asset-recovery`, `feature/local-rag`, `feature/system-care`, `feature/embeddings-hybrid-search`, `feature/d1-encryption-and-vec`, `feature/context-modules-step7`, `feature/put-back-together-and-learning`, & `feature/sets-and-photo-subjects`: merged into `master` (not pushed).
- `checkpoint/pre-stabilization-2026-11-23` (`084e3d5`): rollback tag preserving pre-stabilization baseline plus parked UI refresh docs (`canonical/D1_Design_System_v1.0.md`, `canonical/D3_Critical_UX_Flows_v1.0.md`, `canonical/suite_reconciliation.md`, `design-system/`).
- Leave the two untracked owner docs (`canonical/Aegis Guardian Full PRD.md` and `canonical/local-intelligence-system-architecture.md`) uncommitted and untouched (they keep the old "Aegis" name).
- **Rebrand to Cairn Guardian: complete, merged into `master` and PUSHED (2026-11-25)** to the public repo `https://github.com/MacMbizo/cairn_guardian` (`origin`, only `master` pushed; feature branches stay local). Commits `a20f81d` (user-facing text), `8935511` (on-disk/binary/startup + repo files), `06a7a19` (test fix).
  - Everything the owner sees says Cairn: UI, tray, notifications, window title, executable `cairn-guardian.exe` (`[[bin]]` in `src-tauri/Cargo.toml` + `mainBinaryName`).
  - **Why the owner still saw Aegis:** a stale HKCU `Run` value "Aegis Guardian" launched the 2026-11-23 `target\release\aegis-desktop.exe` at sign-in. `shell::retire_legacy_autostart()` (called from `init_unlocked_settings`) deletes it plus its `StartupApproved` value and, in release builds, registers "Cairn Guardian" if the old entry was enabled; otherwise it re-registers an enabled entry so it follows the renamed exe.
  - Backup folder `Cairn Backup` (`backup::BACKUP_DIR`); legacy `Aegis Backup` is recognised everywhere, `backup_root` falls back to it, and `backup::adopt_legacy_backup_dir` (run at the end of `drives::registry::sync_mounted`) renames it and rebases `files`/history/actions/`backup_copies`/`backup_versions`/`dupe_decisions` in one transaction, renaming back on failure. Test: `legacy_aegis_backup_folder_is_renamed_and_catalog_follows`.
  - Quarantine `.cairn-quarantine` via `quarantine::quarantine_dir_for`: a non-empty legacy `.aegis-quarantine` stays in use (undo paths stay valid) until it empties, then is removed. Policy/scanner/care guards accept both names.
  - `.cairnignore` / `.cairnboundary` markers (legacy `.aegis*` still read); firewall hint uses the running exe path.
  - **Still `aegis` on purpose (invisible to the owner):** crate names, `com.aegis.guardian` app-data folder, `aegis.db` / key / vault files, DPAPI entropy, hidden `.aegis` passport folder, temp suffixes, `aegis://event`, type names, shard token format.
- **Repo files added:** `README.md` (badges incl. CI), `CONTRIBUTING.md`, `SECURITY.md` (private vulnerability reporting enabled), `CHANGELOG.md`, `.github/workflows/ci.yml` (windows-latest: pnpm build, clippy, tests), issue/PR templates. Repo description and topics set.
- `recognise_is_fast_on_a_big_catalog` limit is now 30 s in debug builds (15 s release); it was flaky under full-suite load.

## Project 5 progress (2026-11-25)

- Done: Cairn icon (owner chose option C "Flat precision", `design-system/cairn/`) for app/tray/sidebar/favicon (`92ffb68`); grouped scrolling sidebar + trust rail + bundled IBM Plex Sans / JetBrains Mono (`aaf470d`); plain-language pass removing rule codes from UI, engine messages and fixtures, D1 Home storage bar + health bars + quick-add folders (`99c9f64`).
- Installers built: `target\release\bundle\nsis\Cairn Guardian_0.1.0_x64-setup.exe` and `...\bundle\msi\Cairn Guardian_0.1.0_x64_en-US.msi` (`pnpm dlx @tauri-apps/cli@2 build`; close any running `cairn-guardian.exe` first).
- Waiting on the owner: install and run the checks listed in `docs/PROGRESS.md` §4 (notifications, Ctrl+Shift+Space, tray after reboot, <= 350 MB background).
- UI consistency pass (owner asked for the best, consistent UI/UX app-wide, using `C:\Prj\agent-skills\ui-ux-pro-max-skill` as the rulebook): shared kit in `src/ui/index.tsx` (`PageHeader`, `Card`, `Stat`, `Button` with single-flight `loading`, `Badge`, `Notice`, `EmptyState`, `Tabs`, `ui.*` class tokens); all text >= 12 px; one card surface; normalised buttons; global focus-visible ring, pointer cursor, tabular numbers (`src/index.css`, `@layer base`); every screen has the same header style (`PAGE_META` in `App.tsx` for screens without their own) (`23fb870`, `dc14216`). New UI must use `src/ui`.
- Done since (2026-11-26): plain-language pass on every screen and on engine messages (`6d95465`); `Flash` replaces 21 hand-built banners (`39a3010`); Ctrl+K command bar `src/components/CommandPalette.tsx` (`b245242`); D1 Simulator split view `src/components/SimulatorBatch.tsx` with one "Approve & apply" (only 90%+ policy-allowed changes ticked by default) + immediate Undo, and the undo-conflict resolver `src/components/ConflictResolver.tsx` (`becd51b`); skip-to-content link, focus moves to the new screen on navigation, `.cv-row` (`content-visibility`) on long lists. Preview sample batches: `src/screens/fixtures/actions.ts`.
- Remaining UI work: in-screen tab bars still hand-built (consistent look, but no arrow-key navigation; move to `Tabs`); full virtualisation (`@tanstack/react-virtual`) if real lists prove slow; "Why this destination?" evidence popover and "Create rule" after applying (D1 simulator spec §3–§4); screen-reader pass with NVDA; light theme not planned.
- Bash heredocs in this environment collapse `\\` to `\`: write scripts that contain regex escapes with the Write tool, not heredocs.
- Preview server note: port 1420 is used by another project on this PC; run `pnpm exec vite --port 1430` for the browser preview.

## NEXT CHAT: do this first

Updated 2026-11-26. **All 15 North-Star capabilities are built. The Cairn rebrand, README, CI (green), app icon and the UI consistency pass are done and pushed (`master` = `origin/master` at `9671f9a`).** The installer was rebuilt on 2026-11-26 with all UI work: `target\release\bundle\nsis\Cairn Guardian_0.1.0_x64-setup.exe` (and `...\bundle\msi\Cairn Guardian_0.1.0_x64_en-US.msi`).

Note: closing the window only hides Cairn to the tray. Quit from the tray (or end `cairn-guardian.exe`) before rebuilding, or the build fails because the exe is locked.

The owner was asked to start with list 1. Their results decide what comes next. Otherwise, the owner picks between lists 3 and 4.

**1. Owner checks on the installed build (waiting on the owner)**
- Install the NSIS setup. The cairn icon shows in the Start menu, taskbar and tray.
- Ctrl+K in the app opens the command bar.
- Notifications show under "Cairn Guardian", and clicking one opens the right screen, also from Action Center.
- Ctrl+Shift+Space opens Search from any app.
- After a reboot, Cairn starts in the tray with no window.
- With the window closed, background memory is 350 MB or less.
- Windows CredUI sign-in prompts and BitLocker behave on the real PC.

**2. Real-data checks with drive D: plugged in (owner + Claude)**
- Filing dry run on a `VACUUM INTO` copy of `aegis.db` (`examples/filing_dry_run.rs`): report sample renames and the to-sort share, and fix misclassifications test-first.
- Try Filing for real with the Fresh structure, then undo one batch.
- Phase 1 numbers: catalog time, fingerprint time, file count, duplicate groups, re-plug refresh mode and time.

**3. Remaining UI work**
- In-screen tab bars are still hand-built: they look the same but have no arrow-key navigation. Move them to `Tabs` in `src/ui/index.tsx`.
- D1 simulator spec §3–§4: "Why this destination?" evidence popover, and "Create rule" after applying.
- Screen-reader pass with NVDA.
- Real list virtualisation (`@tanstack/react-virtual`) only if real lists prove slow (`.cv-row` is in place now).

**4. Small deferred issues** (details under "Deferred minor issues" and "Small follow-ups" below)
- The pending navigation slot is set even when the window exists, so "Open Cairn" can land on an old screen.
- Saving an invalid shortcut unregisters the working one, and the tray label hard-codes "Ctrl+Shift+Space".
- The installer-name matcher accepts a first-word-only match for 3+ word names (can wrongly say "already installed").
- Copy fixes: "Paused until 07:00" has no day, the janitor title is fixed, and the only-copy title has no location.
- Search filters only apply to a typed query, and changing a filter needs another press of Search.

**5. Housekeeping**
- Re-run `cargo test --workspace` and clippy after the last UI commits. Only `pnpm build` was verified after them; the last full run was 770 passed.
- `docs/PROGRESS.md` §4 "Vision" list was corrected on 2026-11-26 (all capabilities done).

- Digital Estate Management doc: `docs/superpowers/plans/2026-11-25-digital-estate.md`.
- AI Chief Digital Officer Agent doc: `docs/superpowers/plans/2026-11-25-cdo-agent.md`.
- Personal Data Governance doc: `docs/superpowers/plans/2026-11-25-personal-data-governance.md`.
- Self-Healing Workspace doc: `docs/superpowers/plans/2026-11-24-self-healing-workspace.md`.
- Folderless Mode doc: `docs/superpowers/plans/2026-11-24-folderless-mode.md`.
- Cross-Project Intelligence doc: `docs/superpowers/plans/2026-11-24-cross-project-intelligence.md`.
- Research Librarian doc: `docs/superpowers/plans/2026-11-24-research-librarian.md`.
- Knowledge Extraction doc: `docs/superpowers/plans/2026-11-24-knowledge-extraction.md`.
- Semantic Version Tracking doc: `docs/superpowers/plans/2026-11-24-semantic-version-tracking.md`.
- Forgotten Asset Recovery doc: `docs/superpowers/plans/2026-11-24-forgotten-asset-recovery.md`.
- Local RAG Engine doc: `docs/superpowers/plans/2026-11-24-local-rag-engine.md`.
- Project 4 (System Care) doc: `docs/superpowers/plans/2026-11-24-system-care.md`.
- Plan 6B doc: `docs/superpowers/plans/2026-11-24-embeddings-and-hybrid-search.md`.
- Plan 6A doc: `docs/superpowers/plans/2026-11-24-d1-encryption-and-vec.md`.
- Plan 5 doc: `docs/superpowers/plans/2026-11-23-context-modules-step7.md`.
- Plan 4 doc: `docs/superpowers/plans/2026-11-23-put-back-together-and-learning.md`.
- Plan 3 doc: `docs/superpowers/plans/2026-11-23-sets-and-photo-subjects.md`.
- **Owner Decision D1 approved (2026-11-23) & System Care merged (2026-11-24):** SQLCipher + `sqlite-vec` (`=0.1.9`), DPAPI `R2` + CredUI, 24-word/hex recovery, local embeddings + hybrid search, and non-destructive `K1`/`K2`/`R1`/`R3` Windows System Care.
- **Verification gate on `master`:**
  - `cargo test --workspace`: **768 passed, 0 failed, 2 ignored** (`usn_refresh_on_system_temp`, `live_llama32_grounded_rag_answer`), including all **7** tests in `crates/aegis-core/tests/digital_estate.rs` (`0.15s`), **284** in `aegis-core --lib` (`1.59s`, incl. `estate::shamir::tests`, `estate::engine::tests`, `cdo::patterns::tests`, `cdo::engine::tests`, `governance::detector::tests`, `governance::posture::tests`, `governance::engine::tests`, `healing::markdown::tests`, `healing::tags::tests`, `folderless::compiler::tests`, `folderless::engine::tests`, `cross_project::manifests::tests`, `cross_project::engine::tests`, `librarian::engine::tests`), **7** in `crates/aegis-core/tests/cdo_agent.rs` (`0.23s`), **7** in `crates/aegis-core/tests/personal_data_governance.rs` (`0.21s`), **7** in `crates/aegis-core/tests/self_healing.rs` (`0.30s`), **6** in `crates/aegis-core/tests/folderless_mode.rs` (`0.17s`), **6** in `crates/aegis-core/tests/cross_project.rs` (`0.16s`), **6** in `crates/aegis-core/tests/research_librarian.rs` (`0.15s`), **7** in `crates/aegis-core/tests/knowledge_extraction.rs` (`0.21s`), **7** in `crates/aegis-core/tests/semantic_versions.rs` (`0.24s`), **6** in `crates/aegis-core/tests/forgotten_assets.rs` (`0.19s`), **5** in `crates/aegis-core/tests/local_rag.rs` (`0.10s`), **4** in `crates/aegis-llm` (`0.04s`), **6** in `crates/aegis-core/tests/system_care.rs` (`0.09s`), **6** in `crates/aegis-core/tests/hybrid_search.rs` (`0.17s`), **6** in `crates/aegis-core/tests/d1_encryption.rs` (`1.33s`, `R9` covers `["aegis-core", "aegis-embed", "aegis-llm"]`), **5** in `crates/aegis-embed` (`9.17s`, incl. live `nomic_gguf_live_semantic_check`), **11** in `aegis-desktop` (`0.00s`), **19** in `crates/aegis-core/tests/drives.rs` (`1.80s`), **7** in `crates/aegis-core/tests/core_invariants.rs` (`0.18s`), **7** in `crates/aegis-core/tests/context_modules.rs` (`0.35s`), **46** in `project_plans.rs` (`7.65s`), **97** in `filing.rs` (`9.65s`), **35** in `context.rs` (`16.33s`), **6** in `reunite_and_learning.rs` (`0.44s`), and **11** in `sets.rs` (`0.74s`); `0` folders in `C:\.aegis-quarantine` after verification.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings, `39.16s`); `cargo check -p aegis-desktop --features custom-protocol`: clean (`44.69s`); `pnpm build`: clean (`2430` modules built in `899ms`).
  - Prior Plan 5 live-DB dry run (`examples/filing_dry_run.rs` on a temporary read-only `VACUUM INTO` copy): `239` projects (`84` multi-location), `1` remaining ungathered design set (`MBIZO 1edited` on `D:`), `5` photo subjects, `18` software packages, `9` owner's folders, `4` resources, `52` series seasons, `2` old copies, **`249` plans in `18.8s` debug**.

1. **Owner items still open:**
   - None. The app name is decided: **Cairn Guardian** (2026-11-25).
2. **Note on `src/bindings/` and `node:sqlite`:**
   - Keep `canonical/D1_Design_System_v1.0.md` parked until core features finish.
   - `cargo test` writes LF line endings into `src/bindings/*.ts`; run `git checkout -f --` on unchanged bindings so only real binding diffs are committed.

**What Capability #15 (Digital Estate Management) built:**
- **Encrypted Schema & Crypto Helper (`crates/aegis-core/migrations/0019_digital_estate.sql`, `crates/aegis-core/src/{db/mod.rs,db/crypto.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `estate_trustees`, `estate_shard_records` (metadata only, zero raw shards persisted), `estate_packets`, and `estate_checkins`; makes `fill_random_bytes` `pub`; adds `Screen::Estate`; and exports all 16 TS DTOs (`TrusteeRole`, `EstateTrustee`, `EstateTrusteeInput`, `EstatePacketKind`, `EstatePacket`, `EstatePacketInput`, `EstateShardRecord`, `EstateGeneratedShard`, `EstateShardSetBundle`, `EstateShardVerifyResult`, `EstateHeartbeatState`, `EstateHeartbeatConfig`, `EstateHeartbeatStatus`, `EstateCheckinRecord`, `EstateContinuityExport`, `EstateDashboard`).
- **GF(256) Shamir & Non-Destructive Engine (`crates/aegis-core/src/estate/{mod.rs,shamir.rs,engine.rs}`, `crates/aegis-core/tests/digital_estate.rs`):** Implements pure-Rust GF(256) $K$-of-$N$ `AEGIS-SHARD-V1` secret splitting and Lagrange recovery over `DbKey` with `blake3` checksums and zeroized buffers (`R9`); enforces `R2`/`R7` CredUI gating for packet editing/unmasking, shard generation, quorum drill verification, and non-destructive Continuity Kit exports (`README_TRUSTEE_RECOVERY.md`, `manifest.json`, sealed packet markdowns without leaking raw keys/shards); and drives `R10` non-destructive heartbeat states (`Active`, `Warning`, `GracePeriod`, `ReadyForRelease`).
- **Desktop IPC & `Digital Estate` UI (`src-tauri/src/{commands/estate.rs,commands/mod.rs,lib.rs}`, `src/{screens/DigitalEstate.tsx,screens/fixtures/estate.ts,bindings/client.ts,App.tsx}`):** Exposes all 12 `estate_*` commands (`dashboard`, `save_trustee`, `delete_trustee`, `save_packet`, `delete_packet`, `generate_shards`, `mark_shard_delivered`, `revoke_shard_set`, `verify_quorum`, `save_heartbeat`, `checkin`, `export_continuity_kit`) and adds `DigitalEstate.tsx` with readiness scoring, `Trustees & Shards`, `Quorum Verification Drill`, `Succession Packets`, `Inactivity Heartbeat (R10)`, and `Continuity Kit Export`.

**What Capability #14 (AI Chief Digital Officer Agent) built:**
- **Encrypted Schema & Contracts (`crates/aegis-core/migrations/0018_cdo_agent.sql`, `crates/aegis-core/src/{db/mod.rs,engine/mod.rs,lib.rs,contracts/mod.rs,folderless/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `cdo_recommendation_state`, `cdo_ratifications`, and `cdo_briefing_snapshots` inside `aegis.db`; adds `Screen::Cdo` and `PartialEq`/`Eq` on `FolderlessRuleInput`; and exports all 15 TS DTOs (`CdoPillarId`, `CdoPillarScore`, `CdoBottleneckKind`, `CdoSeverity`, `CdoRecommendationStatus`, `CdoProposalAction`, `CdoProjectPulse`, `CdoVelocitySnapshot`, `CdoBottleneck`, `CdoRecommendation`, `CdoRatificationRecord`, `CdoBriefingCitation`, `CdoExecutiveBriefing`, `CdoScanOptions`, `CdoDashboard`).
- **Executive Synthesizer, `K1` Ratification & Briefings (`crates/aegis-core/src/cdo/{mod.rs,patterns.rs,engine.rs}`, `crates/aegis-core/tests/cdo_agent.rs`):** Evaluates 5 weighted pillars (`Storage & Backup`, `Security & Privacy`, `Workspace Hygiene`, `Knowledge & Alignment`, `Automation Velocity`) strictly from disk/catalog state (`R5`/`R11`); detects per-project code-vs-doc lag (`DocLagBehindCode`) and inbox invoice filing friction (`ManualFilingFriction`); compiles visible Folderless rules (`R6`), Governance privacy rules (`R7`), and reversible schedule setting proposals (`AdjustSetting`) with single-click `undo_cdo_ratification` (`K1`); enforces `K2`, `K4`, and `R7` CredUI unlock gates; and builds cited executive memo prompts (`prepare_cdo_briefing_prompt`) with snapshot persistence.
- **Desktop IPC & `Chief Digital Officer` UI (`src-tauri/src/{commands/cdo.rs,commands/governance.rs,commands/mod.rs,lib.rs}`, `src/{screens/ChiefDigitalOfficer.tsx,screens/fixtures/cdo.ts,bindings/client.ts,App.tsx}`):** Exposes all 7 `cdo_*` commands (`dashboard`, `set_recommendation_status`, `ratify_recommendation`, `undo_ratification`, `synthesize_briefing` off-lock via `OnDemandLlm`, `list_briefings`, `list_ratifications`) and delivers `ChiefDigitalOfficer.tsx` with clickable 5-pillar navigation cards, `Strategic Proposals` (`K1` ratify + single-click undo), `Bottlenecks & Project Pulse`, `R7` unlock toggle, and `Executive Briefings` views.

**What Capability #13 (Personal Data Governance) built:**
- **Encrypted Schema & Bindings (`crates/aegis-core/migrations/0017_personal_data_governance.sql`, `crates/aegis-core/src/{db/mod.rs,drives/registry.rs,engine/mod.rs,lib.rs,contracts/mod.rs,rag/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `governance_label_batches`, `governance_label_entries`, `governance_privacy_rules`, and `governance_file_scans` inside `aegis.db`; rebases `governance_privacy_rules.path_prefix` on drive re-lettering; enforces `is_path_excluded_from_ai` inside `rag::retrieve_rag_context`; adds `Screen::Governance`; and exports all 17 TS DTOs (`GovernanceFindingType`, `GovernanceSeverity`, `GovernanceFindingStatus`, `GovernanceRuleAction`, `GovernanceFindingLocation`, `GovernanceFinding`, `SensitiveFileSummary`, `GovernancePrivacyRule`, `GovernancePrivacyRuleInput`, `GovernanceLabelBatch`, `DriveEncryptionStatus`, `OptInCapabilityAudit`, `PrivacyPostureReport`, `GovernanceScanOptions`, `GovernanceQueryOptions`, `GovernanceScanReport`, `GovernanceDashboard`).
- **Detector, Posture Audit & `K1`/`R7` Engine (`crates/aegis-core/src/governance/{mod.rs,detector.rs,posture.rs,engine.rs}`, `crates/aegis-core/tests/personal_data_governance.rs`):** Pure-Rust detector scores context risk and outputs only `blake3` fingerprints plus strictly redacted snippets across 6 finding types; persists triage (`open`, `vaulted`, `accepted_risk`, `false_positive`, `resolved`) across rescans; excludes `backup` volumes and quarantined/deleted files (`K2`/`K4`); gates sensitive scans/views/staging behind Windows user verification (`R7`); applies reversible `confidential`/`secret` label batches (`apply_sensitivity_labels` + `undo_sensitivity_batch`, `K1`); stages exposed secrets for 30-day Quarantine through `PolicyContext` + `propose_batch` (`K1`/`K2`/`R8`); provides non-admin Windows Shell BitLocker status (`unknown_requires_admin` fallback, `R1`); and reports honest `R8`/`R9`/`R11` guarantees plus PowerShell `New-NetFirewallRule` guidance.
- **Desktop IPC & `Data Governance` UI (`src-tauri/src/{commands/governance.rs,commands/mod.rs,lib.rs}`, `src/{screens/DataGovernance.tsx,screens/fixtures/governance.ts,bindings/client.ts,App.tsx}`):** Exposes all 10 `governance_*` commands (`dashboard`, `scan`, `set_finding_status`, `apply_labels`, `undo_label_batch`, `propose_quarantine` emitting `ActionProposed`, `list_rules`, `save_rule`, `delete_rule`, `apply_rule`) and ships `DataGovernance.tsx` with `Exposures & Secrets`, `Sensitive Vault & Rules`, and `Privacy Posture` (`R1`/`R2`/`R8`/`R9`/`R11`) views, CredUI unlock banner, `K1` reversible classification + Simulator quarantine triggers, and `aegis.file.get` inspection.

**What Capability #12 (Self-Healing Workspace) built:**
- **Encrypted Schema & Move Log (`crates/aegis-core/migrations/0016_self_healing.sql`, `crates/aegis-core/src/{db/mod.rs,db/files.rs,drives/registry.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `heal_patch_payloads`, `heal_move_log`, and `heal_dismissals` inside `aegis.db`; records external OS/USN renames during `upsert_discovered_file` without modifying `file_history` and rebases `heal_move_log` across volume re-lettering; adds `Screen::Healing`, `ActionType::PatchText`, and exports all 12 new TS DTOs (`HealLinkKind`, `HealResolutionStatus`, `HealResolutionSource`, `BrokenLinkItem`, `TagWeedKind`, `TagOccurrence`, `TagWeedCluster`, `RulePathIssue`, `HealScanOptions`, `HealScanReport`, `HealPatchPreviewFile`, `HealBatchSummary`).
- **`K1` Atomic Text Patching & Core Healing Engine (`crates/aegis-core/src/{policy/mod.rs,scanner/mod.rs,db/actions.rs,healing/{mod.rs,markdown.rs,tags.rs,engine.rs}}`, `crates/aegis-core/tests/self_healing.rs`):** Integrates `ActionType::PatchText` into `PolicyContext` (`patch_text_requires_approval`) and `db::actions` with atomic sibling writes (`MoveFileExW`), `blake3` TOCTOU checks on execute/rollback, FTS5/chunk refreshing, and `CopyBackKeepingBoth` post-heal edit preservation. Parses Markdown/wikilinks/embeds/refs outside code blocks, resolves transitive move chains & ancestor dir renames, detects `InQuarantine` (`K2`) & `Ambiguous` targets cleanly without overwriting, consolidates casing & Damerau-Levenshtein tag variants (protecting `#phase1`/`#phase2`), flags `#A3E2F1` hex false positives, enforces `R7` confidential/secret gating and `K4` backup exclusion, and audits stale `folderless_rules` paths.
- **Desktop IPC & `Self-Healing` UI (`src-tauri/src/{commands/healing.rs,commands/mod.rs,lib.rs}`, `src/{screens/SelfHealing.tsx,screens/fixtures/healing.ts,bindings/client.ts,App.tsx}`):** Exposes all 8 `healing_*` commands (`scan`, `preview_patches`, `propose_batch` emitting `ActionProposed`, `dismiss`, `undismiss`, `update_rule_path`, `recent_batches`, `run_doctor_check`) and ships `SelfHealing.tsx` with `Relational Integrity` & `Taxonomic Hygiene` KPIs, broken links & ambiguity candidates, tag weed clusters, stale rule path editor, `D1` DB journal verification, Dry-Run Unified Diff preview, `"Stage in Simulator (K1)"`, `aegis.file.get` modal inspection, and one-click `"Undo Batch"`.

**What Capability #11 (AI Folderless Mode) built:**
- **Encrypted Schema & Contracts (`crates/aegis-core/migrations/0015_folderless_intents.sql`, `crates/aegis-core/src/{db/mod.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `folderless_rules` (`idx_folderless_rules_enabled`) inside `aegis.db`, `Screen::Folderless`, and exports all 6 TS DTOs (`FolderlessRule`, `FolderlessRuleInput`, `CompiledIntentDraft`, `FolderlessMatchPreview`, `FolderlessPreviewReport`, `FolderlessBatchSummary`) via `ts-rs`.
- **Intent Compiler & `K1` Filing Engine (`crates/aegis-core/src/folderless/{mod.rs,compiler.rs,engine.rs}`, `crates/aegis-core/tests/folderless_mode.rs`):** Compiles plain-language intents into inspectable/editable/deletable rules (`{quarter}`, `{year}`, `{org}`, `{kind}`, `{amount}` anchors) with optional off-lock `OnDemandLlm` draft refinement; scans live non-backup files & FTS5 chunks while shielding `atomic_units` (`K4`), excluded quarantined/deleted/missing items (`K2`), and locked `confidential`/`secret` files (`R7`); stages validated relative destinations through `PolicyContext` + `propose_batch` (`K1` Simulator `Move` + single-click `rollback_batch`).
- **Desktop IPC & `Folderless Rules` UI (`src-tauri/src/{commands/folderless.rs,commands/mod.rs,lib.rs}`, `src/{screens/FolderlessMode.tsx,screens/fixtures/folderless.ts,bindings/client.ts,App.tsx}`):** Exposes all 8 `folderless_*` commands (`list_rules`, `compile_intent`, `save_rule`, `toggle_rule`, `delete_rule`, `preview_rule`, `propose_batch`, `recent_batches`) and ships `FolderlessMode.tsx` with plain-language intent composer, editable rule Inspector, dry-run preview table (`match_reasons`, anchor chips, `R7` warning), one-click Simulator batch staging emitting `ActionProposed`, and undoable recent batches.

**What Capability #10 (Cross-Project Intelligence) built:**
- **Encrypted Schema & Bindings (`crates/aegis-core/migrations/0014_cross_project.sql`, `crates/aegis-core/src/{db/mod.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `cross_project_insight_state` table inside `aegis.db`, `Screen::CrossProject`, and exports all 12 TS types (`FindingKind`, `FindingSeverity`, `InsightStatus`, `DependencyKind`, `ProjectNodeSummary`, `CrossProjectFileRef`, `CrossProjectFinding`, `CrossProjectEdge`, `CrossProjectOptions`, `CrossProjectReport`, `CrossProjectBriefCitation`, `CrossProjectBrief`) via `ts-rs`.
- **Cross-Project Analyzer & Manifests (`crates/aegis-core/src/cross_project/{mod.rs,manifests.rs,engine.rs}`, `crates/aegis-core/tests/cross_project.rs`):** Parses `package.json`, `Cargo.toml`, `requirements.txt`, and `pyproject.toml` purely in Rust; detects `DuplicateEffort` across projects (`vec_chunks` cosine, exact `blake3`, or lexical Jaccard while filtering raw manifest noise), `ConflictingDecision` across approved `Decision` entities, and `VersionDivergence` across shared dependency majors; maps 5 kinds of cross-functional dependencies (`SharedStakeholder`, `SharedDecision`, `CrossProjectAction`, `DocumentReference`, `SharedStack`); persists reversible `K1` triage states; enforces `K2`/`K4` exclusions, `R5`/`R11` disk-only analysis, and `R7` confidential/secret gating; and assembles cited executive prompts (`prepare_cross_project_brief`).
- **Desktop IPC & `Cross-Project` UI (`src-tauri/src/{commands/cross_project.rs,commands/mod.rs,lib.rs}`, `src/{screens/CrossProject.tsx,screens/fixtures/crossProject.ts,bindings/client.ts,App.tsx}`):** Exposes `cross_project_scan`, `cross_project_set_insight_status`, and off-lock `cross_project_synthesize_brief`, and adds the `Cross-Project` tab with KPIs, `Overlaps & Conflicts` triage cards + `aegis.file.get` preview, `Dependency Map`, `Projects Overview`, and grounded Local LLM brief synthesis.

**What Capability #9 (Autonomous Research Librarian) built:**
- **Encrypted Schema & Bindings (`crates/aegis-core/migrations/0013_research_librarian.sql`, `crates/aegis-core/src/{db/mod.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `librarian_pins` and `librarian_dismissals` tables inside `aegis.db`, `Screen::Librarian`, and exports all 9 TS DTOs (`ResearchDocItem`, `ResearchDocShelf`, `SharedEntityRef`, `AssociationReason`, `RelatedReference`, `LibrarianOptions`, `ResearchCompanionReport`, `BriefCitation`, `ResearchBrief`) via `ts-rs`.
- **Three-Signal Cross-Reference Engine (`crates/aegis-core/src/librarian/{mod.rs,engine.rs}`, `crates/aegis-core/tests/research_librarian.rs`):** Builds searchable recent document shelves (`list_recent_research_docs`) and discovers related documents either from a saved document on disk (`find_related_for_file`) or an explicit owner-entered topic/excerpt (`find_related_for_topic`, `R5`/`R11`), combining dense `vec_chunks` similarity (`ConceptualParallel` detection for 0 overlapping keywords), FTS5 `chunks_fts` concepts, and Capability #8 approved entities/aliases with cross-project flags, persistent pin/dismiss curation (`pin_reference`, `dismiss_reference`), strict `K2`/`K4` backup/quarantine exclusion, `R7` confidential/secret gating, and `prepare_brief_context` grounded citation assembly.
- **Desktop IPC & `Research Librarian` UI (`src-tauri/src/{commands/librarian.rs,commands/mod.rs,lib.rs}`, `src/{screens/ResearchLibrarian.tsx,screens/fixtures/librarian.ts,bindings/client.ts,App.tsx}`):** Exposes all 8 `librarian_*` commands (running `OnDemandEmbedder` and `OnDemandLlm` behind `rag_running` off the SQLite lock per `K5`/`R4`) and adds the two-pane `Research Librarian` tab with `R5`/`R11` guard indicator, conceptual & cross-project filters, reading-list pins/notes, on-demand reading brief synthesis with clickable `[N]` citations, and read-only `aegis.file.get` inspection.

**What Capability #8 (Automatic Knowledge Extraction) built:**
- **Encrypted Schema & Reuse (`crates/aegis-core/migrations/0012_knowledge_extraction.sql`, `crates/aegis-core/src/{db/mod.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds `entity_aliases`, `knowledge_proposals` (with merge/undo snapshots), and `knowledge_file_runs` inside SQLCipher `aegis.db` while committing approved items into existing `entities` and `relationships` tables; adds `Screen::Knowledge` and exports all 14 TS types via `ts-rs`.
- **Extractor, Incremental Merge & Safety (`crates/aegis-core/src/knowledge/{mod.rs,parser.rs,store.rs}`, `crates/aegis-core/tests/knowledge_extraction.rs`):** Normalizes `.vtt`/`.srt` speaker cues & markdown meetings, extracts `Person`/`Organization`/`Project`/`Decision`/`ActionItem` candidates reusing `filing::orgs::ORGS`, parses `OnDemandLlm` JSON/bullet output, tracks `knowledge_file_runs` watermarks while respecting dismissed decisions, incrementally merges approved entities with full `undo_approved_proposal`, enforces `K2` live provenance filtering, `K4` backup exclusion + default-off audio opt-in, and `R7` confidential/secret skipping + provenance masking.
- **Desktop IPC & `Knowledge Base` UI (`src-tauri/src/{commands/knowledge.rs,commands/mod.rs,lib.rs}`, `src/{screens/KnowledgeBase.tsx,screens/fixtures/knowledge.ts,bindings/client.ts,App.tsx}`):** Exposes all 10 `knowledge_*` commands (incl. off-lock per-file `knowledge_extract_file_llm`), ships the `Knowledge Base` tab with KPIs, `Review Inbox` (`K1` approve / inline edit / batch approve / dismiss / undo), `Knowledge Graph` (aliases, action status toggle, cross-entity links, live source provenance + `aegis.file.get` inspection), and `Transcripts & Audio (K4)` opt-in manager.

**What Capability #7 (Semantic Version Tracking) built:**
- **Schema & Vector Loader (`crates/aegis-core/migrations/0011_version_tracking.sql`, `crates/aegis-core/src/{db/mod.rs,db/vec.rs,engine/mod.rs,lib.rs,contracts/mod.rs}`, `crates/aegis-core/tests/drives.rs`):** Adds encrypted `version_chain_prefs` and `version_diff_summaries` tables, `Screen::Versions`, `le_bytes_to_f32s` / `cosine_similarity` / `load_file_vectors` reading normalized vectors from `vec_chunks`, and exports all 10 version/diff TS types via `ts-rs`.
- **Version Engine, Diffing & `K1` Actions (`crates/aegis-core/src/versions/{mod.rs,patterns.rs,chains.rs,diff.rs,actions.rs}`, `crates/aegis-core/tests/semantic_versions.rs`):** Detects chains across filename iterations (`_v1`/`_v2`/`_FINAL`/dates/`Copy of` while sparing `Lesson_01`-style sequences), `Versions\` folders, `file_history` move/rename lineage, and semantic cosine similarity (`>= 0.92`); flags exact `blake3` duplicates while refusing auto-delete/quarantine of semantic variants (`FR-004.2`); enforces `K2`, `K4`, `R7` (excludes sensitive files from locked similarity/diffs/LLM caching); computes LCS hunks & conceptual section/term badges; stages archive moves (`Move`) and duplicate quarantine (`propose_dupe_quarantine_pairs`, `MIN_COPIES = 2`) with full simulation & rollback.
- **Desktop IPC & `Version Tracking` UI (`src-tauri/src/{commands/versions.rs,commands/mod.rs,lib.rs}`, `src/{screens/VersionTracking.tsx,screens/fixtures/versions.ts,bindings/client.ts,App.tsx}`):** Exposes `versions_scan`, `versions_diff`, `versions_summarize_diff` (reusing on-demand `OnDemandLlm` off the DB lock with encrypted caching), `versions_pin_latest`, `versions_dismiss`, `versions_undismiss`, `versions_propose_archive`, and `versions_propose_quarantine_dupes`; delivers the two-pane `Version Tracking` tab with KPIs, signal filters, ordered timeline, side-by-side & unified diff viewer, local LLM explanation, read-only `aegis.file.get` inspection, and `K1` batch triggers.

**What Capability #6 (Forgotten Asset Recovery) built:**
- **Schema & Format/Header Engine (`crates/aegis-core/migrations/0010_forgotten_assets.sql`, `crates/aegis-core/src/{db/mod.rs,engine/mod.rs,lib.rs,contracts/mod.rs,assets/{mod.rs,formats.rs}}`):** Adds encrypted `asset_meta` and `asset_dismissals` tables, adds `Screen::Assets`, classifies 80+ formats (`PSD`/`PSB`/`AI`/`SVG`/`FIG`/`SKETCH`/`AF*`, RAW camera `CR2`/`NEF`/`ARW`/`DNG`, raster/HDR, `BLEND`/`OBJ`/`FBX`/`GLB`/`USDZ`/`STEP`, `PRPROJ`/`AEP`/video, `WAV`/`FLAC`/`ALS`/audio, `TTF`/`OTF`/`WOFF2`, documents/decks) into 8 `AssetFamily` groups, and parses `PNG`/`JPEG`/`GIF`/`BMP`/`WebP`/`PSD`/`SVG` headers purely in Rust (`R9`).
- **Dormancy Scanner, Collection Matching & `K1` Recovery (`crates/aegis-core/src/assets/recovery.rs`, `crates/aegis-core/tests/{forgotten_assets.rs,drives.rs}`):** Detects buried clutter (`Downloads`, `Desktop`, `OldCopy`), offline-drive assets (`only_copy` via `blake3`/name-size across non-backup drives), and stranded matches to active projects/sets with `suggested_destination`; enforces `K2` (no quarantined/deleted), `K4` (backup drives excluded at SQL level), `R7` (masks snippets and blocks hidden chunk-text queries when locked), and stages reversible `Move` batches via `propose_asset_recovery` -> `build_policy_context` -> `propose_batch` (`K1`).
- **Desktop IPC & `Forgotten Assets` UI (`src-tauri/src/{commands/assets.rs,commands/mod.rs,lib.rs}`, `src/{screens/ForgottenAssets.tsx,screens/fixtures/assets.ts,bindings/client.ts,App.tsx}`):** Exposes `assets_scan` (with optional off-lock `OnDemandEmbedder` hybrid query), `assets_enrich_metadata`, `assets_dismiss`, `assets_undismiss`, and `assets_propose_recovery` emitting `ActionProposed`; ships the `Forgotten Assets` tab with KPI strip, hybrid search, dormancy/drive/family filters, lazy visual header enrichment + color swatches, one-click `"Recover to <Project>"`, bulk folder moves, offline drive notices, read-only `aegis.file.get` inspection, and persistent dismissals.

**What Capability #3 (Local RAG Engine) built:**
- **Pure-Rust Local Chat Inference (`Cargo.toml`, `crates/aegis-llm`):** Implements recursive Ollama manifest + standalone GGUF discovery (`registry.ollama.ai` and `hf.co`, marking `<= 4,500 MB` as `recommended` with `builtin:extractive` fallback), pure-Rust GPT-2 BPE (`llama-bpe`, `qwen2`) + SentencePiece tokenizer with buffered UTF-8 streaming, quantized CPU `GgufLlm` supporting `Llama` (interleaved `rope_i`, `rope_freqs.weight`, tied LM head) and `Qwen2` (split `rope`, QKV bias) with KV cache, `ExtractiveGenerator`, and `OnDemandLlm` (`K5`: immediate unload for `> 2.5 GB` models, `60s` idle unload for compact models, cooperative cancellation).
- **Core Retrieval, `R7` Privacy Gate & Contracts (`crates/aegis-core/src/{rag/mod.rs,engine/mod.rs,contracts/mod.rs,lib.rs}`, `crates/aegis-core/tests/{local_rag.rs,d1_encryption.rs}`):** `retrieve_rag_context` strips question stopwords and widens across content terms for natural-language FTS5/hybrid questions, forces `include_quarantined = Some(false)` (`K2`), excludes sensitive files by default while counting `excluded_sensitive_count` without loading chunks (`R7`), rejects `include_sensitive = true` unless Windows CredUI unlock is active, exports `LlmModelInfo`/`LlmStatus`/`RagCitation`/`RagAskOptions`/`RagAnswer` + `Screen::Ask` + `AegisEvent::RagToken`, and extends `R9` networking-crate ban to `aegis-llm`.
- **Desktop IPC & `Ask Files` UI (`src-tauri/src/{commands/rag.rs,commands/mod.rs,commands/vault.rs,engine_host.rs,lib.rs}`, `src/{screens/AskFiles.tsx,screens/fixtures/rag.ts,screens/EngineSettings.tsx,bindings/client.ts,App.tsx}`):** Exposes `rag_status`, `rag_select_model`, `rag_unload`, `rag_cancel`, and `rag_ask` with token streaming, restores `rag.model_path` on startup/unlock, hooks `60s` idle unload in `engine_host` and immediate unload on `WindowEvent::Destroyed`, adds the `Ask Files` tab with live streaming, `R7` toggle, inline `aegis.file.get` inspection, and adds the **Local RAG Chat Model (`R4` / `K5`)** card to Settings.

**What Project 4 (System care) built:**
- **Core System Care Engine (`crates/aegis-core/src/{care/*,policy/mod.rs,db/actions.rs,engine/janitor.rs,engine/mod.rs,engine/note.rs,engine/watches.rs,contracts/mod.rs}`):** Adds `care::cleanup` (`TempFiles`, `CrashDumps`, `BrowserCache`, `DevCache`, `BuildCache`, `DownloadsJanitor`, `DeadFiles` protecting projects/atomic units/backups, plus `propose_care_quarantine` indexing uncatalogued files safely into 30-day quarantine), `care::apps` (`KEY_READ`-only uninstall inventory, registry-backed `lookup_uninstall_command` + `parse_uninstall_invocation` stripping `/qn` for `msiexec`, protected vendor guard `is_safe_leftover_name`, `scan_leftover_folders` -> `UninstallLeftover`), `care::startup` (read-only `Run`/`RunOnce`/`StartupApproved`/Startup folders + orphan detection), `care::smart` + `SmartHealthWatch` (`Screen::Care`, `"drive_smart"` alert on warnings/predictive failure/>=70°C/>=90% wear), `care::trends` (30d/90d `space_samples` & `root_samples` deltas + forecasts), narrow `PolicyContext` allow-lists (`care_cleanup_roots`, `care_disposable_dirs`, `care_leftover_parents`), tightened 2-word `installer_matches`, upgraded 5-dimension `health_breakdown`, and all 6 integration tests in `tests/system_care.rs`.
- **Desktop IPC & UI (`src-tauri/src/{commands/care.rs,commands/mod.rs,lib.rs,engine_host.rs}`, `src/{screens/SystemCare.tsx,screens/fixtures/care.ts,App.tsx,bindings/*}`):** Exposes all 10 `care_*` commands off the UI DB lock (with `ShellExecuteW` UAC fallback for elevation-required uninstallers and `ms-settings:appsfeatures` / `ms-settings:startupapps`), registers `SmartHealthWatch`, adds TS client bindings (`aegis.care`) & preview fixtures, and ships the 6-section `System Care` tab (`Overview & Health`, `Cleanup`, `Apps & Leftovers`, `Startup Items`, `Drive Health (SMART)`, `Disk Trends`).

**What Plan 6B built:**
- **Pure-Rust On-Demand Embedder (`Cargo.toml`, `crates/aegis-embed`):** Implements `WordPieceTokenizer` (both HF `##` and GGUF `\u{2581}` conventions), `GgufEmbedder` (`nomic-bert` NeoX RoPE/SwiGLU + standard `bert` via compact `QMatMul` on `candle-core` + `candle-nn`), `SafetensorsBertEmbedder`, `LexicalHashEmbedder` (`384`-d fallback), local model discovery (app `models/` + local `.ollama/models` GGUF headers preferring `<= 350 MB` per `K5`), and `OnDemandEmbedder` unloading immediately after batch embedding or on idle/`unload()`, keeping `r9_forbids_networking_crates_in_engine_and_models` clean.
- **Encrypted Vector Sync, Hybrid RRF Search & AC Gating (`crates/aegis-core/src/{db/vec.rs,db/search.rs,db/files.rs,doctor/mod.rs,engine/mod.rs,contracts/mod.rs,filing/mod.rs}`):** Manages `vec_chunks` virtual tables (`sqlite-vec` `0.1.9`) inside SQLCipher `aegis.db`, increments `pending_chunks` via BLAKE3 content hashes, marks chunk outbox events processed, prunes deleted/quarantined orphan vectors (Doctor Check #4), fuses FTS5 BM25 + vector KNN + filename + former-name channels (`search_hybrid` with RRF `k = 60`, collection filtering/boost, drive online/offline state, `K2` quarantine exclusion, `R7` masking), adds `Watch::needs_ac()` to gate `EmbedWatch`, and passes all 6 integration tests in `tests/hybrid_search.rs`.
- **Desktop IPC, Watcher & Search/Settings UI (`src-tauri/src/{lib.rs,engine_host.rs,commands/{embed.rs,mod.rs,vault.rs}}`, `src/{App.tsx,components/SearchFilterBar.tsx,screens/EngineSettings.tsx,bindings/*}`):** Exposes `embed_status`, `embed_sync_now`, `embed_select_model`, and `embed_unload`, schedules AC-gated `EmbedWatch` with `60s` idle unload + `store.shrink_memory()`, unloads weights on `WindowEvent::Destroyed`, restores saved `embed.model_path` on unlock, adds `Project / Set` filtering + `Hybrid`/`Keyword`/`Semantic` toggles + match-reason/unplugged-drive badges to Search, and adds the **Semantic Search & Local Embeddings** card in Settings.

**What Plan 6A built:**
- **SQLCipher + `sqlite-vec` Foundation (`crates/libsqlite3-sys-sqlcipher`, `Cargo.toml`, `migrations/0009_vault_and_vec_meta.sql`):** Compiles SQLCipher `sqlcipher-src 420.0.0` + `libtomcrypt 1.18.2` with `-DLTC_NO_ASM` directly under MSVC (zero Perl/OpenSSL dependency), registers `sqlite-vec = "=0.1.9"` on every `SqliteStore`, and creates `chunk_embeddings_meta`.
- **Key Sealing, Offline Recovery & Plaintext Migration (`db::crypto`, `db::mod`, `src-tauri/lib.rs`, `commands/vault.rs`, `engine_host.rs`, `watch.rs`):** CSPRNG 256-bit `DbKey` (`ZeroizeOnDrop`), Windows DPAPI sealing (`aegis.key.dpapi`, `R2`), verified `sqlcipher_export` migration removing plaintext once row counts + FTS5 + `PRAGMA quick_check` verify (`R8`), 24-word BIP-39 / 64-hex recovery (`aegis.vault.json`), Windows CredUI (`CredUIPromptForWindowsCredentialsW`) timed sessions for sensitive views and recovery key reveal, and `AppState::open_store()` across all desktop handlers and background watchers.
- **R7 Sensitive Views, UI & Tests (`db::files`, `doctor`, `EngineSettings.tsx`, `App.tsx`, `tests/d1_encryption.rs`):** Indexes confidential/secret files for search while masking snippets/previews (`SENSITIVE_MASK`) unless unlocked; Settings `Security & Encryption` card with 1-based spot-check confirmation + CredUI reveal + 15-min timer; locked-vault recovery screen + unconfirmed backup banner + Search sensitive toggle; 6 integration tests incl. `Cargo.lock` `R9` networking crate guard.

**What Plan 5 built:**
- **Step 7 Modules (`context::{software,owners,resources,series,old_copies,recognise}`):** Software packages (with `INTERNAL_SW_DIRS` composite-installer preservation into `Software\Tools`/`Installers`), Owner's folders (purpose-name + content/metadata agreement, `.db` pinning, CV-in-Work overrides), Resources (`SKILL.md`, `DESIGN.md`, templates/kits into `Documents\Resources`), Series (`Videos\Shows\<Show>\Season <n>` + companion subtitles), and depth-1 Old copies (`D:\MSIDocuments`, `D:\MSIDownloads`) claimed before projects and compared against live folders.
- **External Drive & Archive Plans (`context::plan`, `filing::{blueprint,batches}`):** `StepKind::ToArchive` moves + same-drive quarantine with BLAKE3 proof, external-drive per-volume planning (same-drive comparison only, offline `waits_for`), `Planner` `offline_dirs`/`projects_homes`/`summaries` caches (cutting 249 plans from `988.0s` to `18.8s` debug), `OldCopy` preservation over inner `atomic_units` in `blueprint::find_groups`, and UI `OldCopy` filter/actions in `Projects.tsx`.

**What Plan 4 built:**
- **Reunite Engine (`context::reunite`, `context::plan`, `filing::batches`):** Queries earliest filing `file_history` rows for live files, traces executed `MoveHome` ancestor folders (`rev_moves`, cycle-guarded), claims split files with priority `Project > DesignSet > PhotoSubject`, proposes collision-safe exact-dest moves (`propose_reunite`), runs atomically (`run_reunite` syncing `collection_members` & recording `"photo:<subj>"` `LearnedSubject`), wires `"Put back together: <Name>"` into `recent_batches`, and undoes cleanly via `filing_undo` (`undo_reunite`).
- **Learned Name Aliases (`context::relate`, `context::recognise`):** `learned_aliases` replays `SameAs`/`NotSameAs` chronologically over normalized names (`>= 3` chars, excluding area/life-area/part noise words) with union-find, feeds `a:<group>` candidate index keys, and upgrades `identity_of_with_aliases`.
- **Weekly Note Summary (`engine::note`):** Adds `#[serde(default)] pub projects: Option<String>` populated from active multi-location project counts + `reunite_plans_conn`, preserving backwards-compatibility with legacy saved JSON rows.
- **UI & IPC (`commands/context.rs`, `filing.rs`, `Projects.tsx`, `ThisWeek.tsx`, `Filing.tsx`, `App.tsx`):** `context_reunite_plans`, `context_run_reunite`, `"Put back together"` top section with split-batch citation & Undo, `"Same project…"` picker on project cards, `"Projects"` section + Review link on `ThisWeek`, and `initialView` routing into Filing/Projects/Recent.

**What Plan 3 & Design-vs-Software differentiation built:**
- **Schema & Store:** Migration `0008_sets.sql` (`collections.fingerprint`, `collection_members`), `store::save_collections_with_members` (keeping set members out of `collection_locations`), `synthesize_set_locations`, live-path `list_members_with_collection`, `drives::registry::rebase_volume_paths` rebasing `collection_members.path`.
- **Recogniser (`context::sets`, `projects`, `recognise`, `answers`):** Multi-format artwork matching, false-positive filters, `DesignSet`/`PhotoSubject` classification, `DecisionKind::ProjectKind` + `Answer::SetKind`, `Ctx::is_kind_shelf` treating `<projects home>\Design` as a Design shelf while leaving nested `<CodeRepo>\Design` folders untouched, `ASSET_EXTS` expanded to 9 formats, `RecogniseReport::{design_sets, photo_subjects}`.
- **Plans & Filing (`context::plan`, `db::actions`, `filing`):** `StepKind::GatherFiles`, `kind_home` (`C:\Prj\Design\<Name>` for Design vs `C:\Prj\<Name>` for Software/Writing/Research/Other), `eligible_home_move` + kind-aware `already_home`, `follow_moves` preserving explicit `project_kind`, `unfollow_moves` + `after_undo` cleaning empty `Design` shelf parent folders, cross-root hoisted `protected_files`/`photo_subjects`, `LearnedSubject` catch on new single arrivals.
- **UI & Build (`src-tauri/Cargo.toml`, `Projects.tsx`, `fixtures/projects.ts`):** `[features] custom-protocol = ["tauri/custom-protocol"]` for standalone `dist/` builds, `DesignSet` IPC support, color-coded `KindBadge` icons, Design fuchsia left accent, kind filter pill bar with counts, inline `<select>` reclassification, `SmoothLite` fixture + updated `ts-rs` bindings.

**Earlier baseline (merged to `master` at `76708b8`):**
- Plan 1, Plan 2 (project plans), and post-Plan-2 stabilization (`c63e967`, `b9899b2`, `6a4afde`, doc reconciliation): `docs/superpowers/plans/2026-11-23-project-plans.md`.

**What Plan 2 built:**
- **Planner** (`context::plan`):
  - one plan per project per drive;
  - the main copy moves whole into `C:\Prj`;
  - identical and older copies go to quarantine. Every file is proven by BLAKE3 at run time; otherwise the whole copy is kept as a version;
  - copies changed on both sides, and earlier versions, go to `Versions\<date> <origin>`;
  - material goes to `Docs`, `Design` or `Material`;
  - a git repo home gets a sibling `<name> - material` folder;
  - a copy at the target name is moved aside first;
  - cards are ordered safest first (`rank`).
- **Executor** (`db/actions.rs`):
  - step order;
  - stale-folder check;
  - exact destinations (never ` (2)`);
  - a project batch stops at its first failure, and `plan::run` undoes it;
  - the catalog, `atomic_units` and `cache_dirs` follow moved folders at once;
  - undo replays chained steps in reverse;
  - folder quarantine takes its files' catalog state with it.
- **UI** (owner's design A):
  - a plan line under each place;
  - Keep here / Include in plan;
  - Change main copy;
  - Approve plan, which runs at once with Undo on the card;
  - a Move button on single projects in the compact list;
  - Recent batches shows "Project plan: <name>".
  - IPC: `context_plans`, `context_run_plan` (single-flight with recognition), `filing_undo` routing to `plan::undo`.
- **Rulings from the owner's real data:** they only make plans move less.
  - Nothing inside another project (or holding one) moves.
  - Python environments never move. Moving a folder that holds one warns.
  - Other drives get no plan yet (the Old copies module comes later).
  - A home moves in only from inside a projects folder.
  - Copies changed in the last 30 days stay.
  - The home keeps its own folder name.
- **Dry run on a copy of the live database:** 51 plans in 1.3 s. 41 homes move in; 1 copy goes to quarantine; 21 are kept as versions; 1 folder of material is brought in.
- **Rehearsal** (`examples/plan_rehearsal.rs`) on copies of the real forexTrader and DrivingSchool (117,044 files): both ran, and undo restored every file exactly.
- **Verified (post-stabilization):** `cargo test --workspace` **476 passed, 0 failed, 1 ignored** (`usn_refresh_on_system_temp`), with both timing tests (`recognise_is_fast_on_a_big_catalog`, `migration_speed_and_fts_sync`) passing and **0** new folders leaked into `C:\.aegis-quarantine` (`57` before / `57` after). Clippy and `pnpm build` are clean.
- **Nothing has moved on the owner's real folders.**

**Noticed, not fixed:**
- *(Resolved 2026-11-23 in `b9899b2`, `6a4afde`: `aegis-desktop` background RAM/CPU churn and real `C:\.aegis-quarantine` test leaks.)*
- UI baseline docs (`canonical/D1_Design_System_v1.0.md`, `canonical/D3_Critical_UX_Flows_v1.0.md`, `canonical/suite_reconciliation.md`, `design-system/`) are saved at `checkpoint/pre-stabilization-2026-11-23` (`084e3d5`). Leave untracked `canonical/Aegis Guardian Full PRD.md` and `canonical/local-intelligence-system-architecture.md` uncommitted.
- Copying the live database: no `sqlite3` or Python on this PC. Use `node` with `node:sqlite` (read-only `VACUUM INTO`); the script lived in the session scratchpad.

**North star:** `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md` holds the binding rules for every new spec or plan.

Ideas backlog below: ask the owner which to design after Plan 2.

**What Plan 1 built** (`crates/aegis-core/src/context/`):
- `profile` (per-folder stats from the catalog), `projects` (recogniser), `relate` (identity, relationships, fingerprints),
  `recognise` (full pass over PC roots and every catalogued drive except Backup), `answers`, `store`, `watch` (`ContextWatch`,
  runs before `FilingWatch`, at most every 10 min when the catalog or answers changed). Migration 0006.
- **Filing never splits a project:** `find_groups` keeps every project root (confidence 0.60 or more) whole; waiting batches are
  re-checked (`recheck_open_batch`, moves inside projects become `superseded`). The owner was told filing is safe to resume.
- **Projects screen** under Filing (owner chose layout C with B's highlights): confirm list, multi-place cards, compact list.
  IPC `context_projects`, `context_recognising`, `context_recognise_now` (single-flight), `context_answer`.
- Dry run (`filing_dry_run`) prints projects per container and every multi-place project with relationships.

**Measured on a copy of the owner's database (2026-11-22):** 253 projects, 86 in more than one place, 17.5 s (release);
14 of 14 turned-down project moves protected. Verification: `cargo test --workspace` 418 passed, 1 ignored (USN on C:);
clippy clean; `pnpm build` clean; Projects screen checked in the browser preview.

**Tuning rulings made on real data (owner may overrule):**
- Files and declared names found in projects of 3+ different names are boilerplate (starter kits): ignored for identity.
- Differently named folders are one project only with 80% of the smaller copy in common; same names need one distinctive file.
- "Possibly the same project" (name only) pairs only two folders that have nothing else.
- A drive's top level, Documents, Downloads and Pictures never lend "sits among other projects"; other containers lend it only
  when 40% of their sizeable sub-folders are projects by themselves. In such a container, a folder of 3+ files is asked about (0.60).
- An answer names one folder (same drive by id, same path or nearly the same files), never its copies.

**Known, not done:**
- Code exercise folders inside course folders (Udemy) count as small projects, so those courses are not moved to Videos\Courses.
- Loose files directly inside a container (e.g. `Dev Set` itself) are still filed by Fresh (spec §9, 2nd bullet).
- `D:\xampp`, Python venvs (`env38`), `Downloads\DDU …` are kept whole as "projects": right for protection; they belong to the
  later Software packages module. `D:\MSIDownloads` holds a `.git`, so it is a repo.
- On a card, "files" counts all non-build files while "shared" counts files of 1 KB or more; the copy is phrased from the
  relationship, not the numbers.
- The app shell is not responsive at phone width (pre-existing; the Projects screen itself is fluid).

**Owner's framing (2026-11-22):** *"Do not be blinkered into making the system about projects … it is a complete system for my PC."*
  - Projects is **one module** of a whole-PC context layer (spec §1A).
  - The other modules follow later, each with a short design of its own: Software packages, Owner's folders, Resources, Series, Photo subjects, Sets, Old copies.
- **Owner decisions in the spec:**
  - A git repo may move **whole** into `C:\Prj` when the owner approves its plan. The card warns about `.venv` and recent changes.
  - **All** projects, software or not (e.g. SmoothLite artwork), live in the one projects home, `C:\Prj` on the PC.
- **Measured facts the spec relies on (spec §2):**
  - C:\Prj is a root.
  - Copies of the same project relate in different ways: forexTrader (the `C:\Prj` copy is an older subset), DrivingSchool (changed on both sides), paperPilot (different material), wellness (no overlap).
  - 14 of the owner's 15 rejected moves (all except a Downloads `.zip`) moved project files by kind. They become regression tests.

## Owner's vision document (read 2026-11-22)

**2026-11-23:** the review below is now recorded as binding rules and a build order in
`docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`. Every new spec and plan must add a
"North-star rules" section. Owner decision D1 (encryption at rest and key handling) is due before the embeddings work.

`canonical/local-intelligence-system-architecture.md` (the owner's; untracked, **never commit it**) describes the 14 backlog
ideas below as one local-first "intelligence OS". Claude's review, given to the owner:
- Treat it as the north star, mapped onto Cairn, not a restart: Cairn already has the USN journal (read **without** admin),
  SQLite store, idle-gated scheduler, content reader, batches with undo, duplicates, the janitor, and now the context layer.
- Decide early (before more data piles up): encryption at rest and key handling. A key that needs Windows Hello would stop the
  tray engine (tripwire, backup) after a reboot; prefer a DPAPI/TPM-sealed key for the engine, biometrics only to open sensitive views.
- Avoid or reshape: direct MFT parsing (needs admin/elevation); multi-pass shredding (ineffective on SSDs; use BitLocker);
  a dead-man's switch that destroys data (release only, never auto-shred); registry cleaning; Recall-style screen capture;
  PII masking of the search index (would break finding one's own documents); "mathematical" zero-network claims.
- LLM routing ("Folderless") should compile plain-language intents into visible rules that feed the existing
  propose/approve/undo pipeline, not decide per file. Local LLMs run on demand only (the 350 MB background budget stands).
- Suggested order: Plan 2 (project plans), then embeddings + hybrid search (aegis-embed + FTS), then local RAG over it,
  then dead files / health score (largely exists), then knowledge extraction, then cross-project intelligence;
  CDO agent and estate management last. Each gets a short design of its own, as before.

## Ideas backlog (owner asked about these, 2026-11-22)

AI Folderless Mode, Digital Estate Management, Automatic Knowledge Extraction, Personal Data Governance,
Cross-Project Intelligence, Semantic Version Tracking, Dead File Detection, Forgotten Asset Recovery, Local RAG Engine,
Autonomous Research Librarian, Self-Healing Workspace, Context-Aware Search, Workspace Health Score, AI Chief Digital
Officer Agent.

- Each one was explained, with how it maps onto Cairn, in the last message of the design chat.
- Most build on the context layer, so it comes first.
- Not yet scheduled. Ask the owner which to design next once Plan 1 lands.

## Where things stand (2026-11-22)

- **Branch:** `master` only. Projects 1, 2 and 3 are all merged. Nothing is pushed.
  - The only untracked file is `canonical/Aegis Guardian Full PRD.md`. It belongs to the owner: **never commit it.**
- **Project 3, Auto-filing:** merged to `master` on 2026-11-22 (fast-forward to `fa3f120`), after 13 of the 14 deferred minor fixes:
  collision suffix ` (2)`, local-time dates, Windows reserved names, Tidy keeps digits, per-item folder suggestions
  under Documents, honest cached counts ("Plug in to see"), **Recent batches** with Undo (linked from the weekly note),
  home tile file count, safe change of structure mid-way, plain failure reasons, dry-run refuses the live database,
  the Backup drive is never read, and search filters (kind, category, organisation, year, drive).
  - Design: `docs/superpowers/specs/2026-11-21-auto-filing-design.md`; plan: `docs/superpowers/plans/2026-11-21-auto-filing.md`.
- **Verification on the merged master:** `cargo test --workspace` green (374 passed, 1 ignored on purpose: the USN test on C:),
  `cargo clippy --workspace --all-targets` clean, `pnpm build` clean. The Filing screens were checked in the browser preview.
- **Dry-run tool:** `cargo run --release -p aegis-core --example filing_dry_run -- <COPY of aegis.db>`.
  Make the copy with the SQLite backup API first. It now refuses the live database and any app data folder.
  Its times are UTC (the core only knows local time when the desktop sets it).

### Rulings made in the minor-fix pass (owner may overrule)

- Collisions start at ` (2)` for **every** move and copy-back, not only filing (one convention, like Explorer).
- Screenshots are renamed only when the date comes from the name, EXIF or content, never from the modified date.
- Reserved names get a trailing `_` (`CON` → `CON_`); empty music tags fall back to "Unknown artist/album".
- Undo, Skip and new arrivals clear the cached counts; they are re-planned on the next visit.
- Undone moves stay "declined" (existing design), so undo does not offer those files again.
- Changing structure mid-way marks the waiting batch `superseded` (not a "no" for the files or their rules).
- The weekly note links "Filed N files by itself" by matching that wording.

## Do next, in this order

0. **Plan 2 of context-aware filing** (project plans; see the top of this file). Plan 1 is done.
1. **Dry run with the external drive (deferred item 14).** Plug in D:, copy `aegis.db`, run the dry-run tool on the copy.
   Report sample renames and the to-sort share; fix misclassifications test-first.
2. **Try Filing on the PC**, starting with the Fresh structure; then Recent batches → Undo once to see it work.
3. **Owner checks that need the installed build.** Quit the running `target\release` Cairn first. Then:
   1. `pnpm tauri build`, then install the NSIS installer.
   2. Notifications show under "Cairn Guardian"; clicking one opens the right screen, also from Action Center.
   3. Ctrl+Shift+Space opens Search. After a reboot, Cairn starts in the tray with no window.
   4. Background memory with the window closed is at most 350 MB.
4. **Phase 1 numbers.** Re-plug D: and report catalog time, fingerprint time, file count, duplicate groups, re-plug refresh mode and time.
5. Then Project 4 (System care).

### Small follow-ups noticed while fixing (not done)

- Search filters only apply to a typed query (the backend returns nothing for an empty query), and changing a filter
  needs another press of Search.
- The filing watch only notices arrivals on scopes that have had a batch approved; new files on a re-plugged drive
  don't clear its cached counts.

## What Project 2 does

- **Running all the time:**
  - Tray icon (the shield with a status dot), and a single instance.
  - Starts with Windows (release builds only).
  - Closing the window keeps the engine running; the webview unloads.
  - Ctrl+Shift+Space opens search from anywhere.
- **Two engine threads, each with its own database connection:**
  - Event thread: live index of the PC roots and the ransomware tripwire.
  - Scheduler thread, below normal priority. It runs the watches:
    - only-copy alarm
    - quarantine expiry
    - drive read errors
    - self-check
    - space recorder and forecast
    - Downloads janitor (proposals only)
    - tripwire lock reminder
  - The scheduler also builds the weekly note, delivers reminders and sets the tray state.
- **Reminders:**
  - "Now" reminders show at once.
  - "When convenient" reminders: at most one a day, held while the owner is busy or Cairn is paused.
  - Dismissing a kind of reminder 3 times moves it to the note.
  - Clicking or closing a notification counts as acting on it or as "not now".
- **UI:**
  - A **This week** screen and a **Settings** screen.
  - A tripwire banner with "It was me". It also appears after a restart while a folder is locked.
  - A "This week" link and a pause chip on Home.

### Measured (debug build, scratch database)

- Window closed: 33–56 MB, 0 webview processes.
- A second launch hands over to the running copy.
- Live test with 300 files:
  - Creating them: indexed, no alarm.
  - Encrypting them to `.locked` and deleting the originals: alarm, lock and notification.
- Janitor dry run on the owner's real Downloads (proposals went to a scratch database): 7.5 GB found, including 25 installers for programs already installed.

## Deferred minor issues

**From the Project 2 final review:**

- The pending navigation slot gets set even when the window already exists, so a later "Open Cairn" can land on an old screen.
- The tray label hard-codes "Ctrl+Shift+Space". Saving an invalid shortcut also unregisters the working one.
- `mark_interrupted_jobs` runs after the other threads have started (a small race at startup).
- The installer-name matcher accepts a match on the first word alone for names of 3+ words, so it can mislabel an installer as "already installed".
- A Windows rename counts both its From and To events toward the tripwire's 200-file threshold.
- Copy fixes:
  - "Paused until 07:00" doesn't say which day.
  - The janitor title is always "old installers and repeat downloads".
  - The only-copy title doesn't say where the files are.
- A locked root's halt cards read "(0 of N)" and repeat for each sub-folder.
- A busy error while reading `watch_state` or writing reminders aborts the rest of that tick.
- The engine's 24 h grace clock for heavy first runs lives in memory, so it resets when the app restarts.
- Clicking a notification from Action Center after the popup has gone probably lands on Home. Check this on the installed build.

**Still open from Project 1:**
*(Resolved in `6a4afde`: `.aegis-partial` on-disk BLAKE3 re-hash, `path_hash` `volume_id` filter, duplicate keeper radio `keeper_index` wiring, and extended `rebase_volume_paths`.)*

- A `backup_jobs` row stays `running` after an error.
- The first top-up isn't shown for approval.
- There are two verified-copy implementations.
- `index_paths` applies a large USN replay in one transaction. The live index now batches 500 paths per transaction, but the USN refresh path doesn't yet.

## Programme (owner-approved order)

1. Drives & duplicates (done, merged)
2. Always-on engine (done, merged)
3. Auto-filing (done, merged 2026-11-22):
   - blueprint choice
   - learn-then-take-over
   - naming as `<date> <who/what> <type>`
   - migration in batches
4. System care: temp files, caches, installers, unused apps, startup items, SMART drive health, disk-space trends. Space history is now recorded daily by the engine.
5. Visual app, which grows alongside the others.

Owner decisions:

- learn-then-take-over autonomy
- deletes always ask and go to the 30-day quarantine
- one Backup drive, kept as a mirror with a 90-day window
- start with the PC folders
- a command-centre home
- **reminders, not interruptions ("just a reminder is fine")**
- **the "I didn't know I needed this" factor**

## Working notes for this machine

- **Shell text gets mangled.** Heredocs and Python heredocs halve backslashes. Write Rust/TS with Write or Edit; heredocs only for content without backslashes.
- **Line-ending noise in bindings.** `cargo test` rewrites `src/bindings/*.ts` with LF. Commit only files with real content changes (`git diff -w --ignore-cr-at-eol`).
- **Test runs never touch the owner's data.** Set `AEGIS_DATA_DIR=<scratch>` to give the app its own database and WebView2 profile. Without it, a second copy fails with WebView2 0x800700AA while the owner's copy runs.
- **A background git (the editor) briefly holds `.git/index.lock`.** Batch git commands and retry.
- **A Vite server is often already running on 1420.** Reuse it.
- **The owner often runs `target\release\aegis-desktop.exe`.** Stage only your own files, and don't rebuild release while it runs.
