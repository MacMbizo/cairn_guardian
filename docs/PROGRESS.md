# Aegis Guardian: progress and what remains

Updated 2026-11-24. This is the whole-project view. `HANDOFF.md` covers the working detail for the next chat.

## 1. Where the code is

| Branch / Tag | State |
|---|---|
| `master` | Projects 1–3, Context-aware filing Plans 1–5, **Plan 6A** (SQLCipher encryption, `sqlite-vec` 0.1.9, DPAPI `R2`, recovery, `R7`, `R9`), **Plan 6B** (local `aegis-embed`, encrypted `vec_chunks`, hybrid RRF search, AC-gated `EmbedWatch`), **Project 4** (Windows System Care: cleanup & dead files, `R3` read-only apps/startup + left-over quarantine, SMART watch, disk trends, 5-dimension health score), **Capability #3: Local RAG Engine** (pure-Rust `aegis-llm` GGUF `Llama`/`Qwen2` + extractive fallback, `core::rag` `K2`/`R7`, streaming `Ask Files`), **Capability #6: Forgotten Asset Recovery** (`core::assets` 80+ formats & visual header cache, offline-drive & project matching, `K1`/`K2`/`K4`/`R7`, `ForgottenAssets`), **Capability #7: Semantic Version Tracking** (`core::versions` chains, `vec_chunks` similarity, LCS/conceptual diff, `OnDemandLlm` summary cache, `K1`/`K2`/`K4`/`R7`, `VersionTracking`), **Capability #8: Automatic Knowledge Extraction** (`migrations/0012`, `core::knowledge` typed entities/transcripts, `K1` inbox + incremental merge + undo, `K4` opt-in audio, `R7`, `KnowledgeBase`), **Capability #9: Autonomous Research Librarian** (`migrations/0013`, `core::librarian` `vec_chunks`/FTS5/Knowledge Graph cross-referencing, `R5`/`R11` disk-only, `ResearchLibrarian`), **Capability #10: Cross-Project Intelligence** (`migrations/0014`, `core::cross_project` duplicate effort, conflict/version drift & dependency mapping, `R5`/`R7`/`R11`, `CrossProject`), **Capability #11: AI Folderless Mode** (`migrations/0015`, `core::folderless` plain-language intent compiler into visible/editable/deletable rules `R6`, content/quarter/org/amount extraction, `K1` Simulator `Move` batches + rollback, `K2`/`K4`/`R7`, `FolderlessMode`), and **Capability #12: Self-Healing Workspace** (`migrations/0016`, `ActionType::PatchText` atomic `MoveFileExW` + TOCTOU/rollback, `core::healing` Markdown/wikilink + YAML/inline tag consolidation + hex color audit + Folderless rule path audit, `K1`–`K5`/`R1`/`R7`/`R8`/`R9`/`D1`, `SelfHealing`), fast-forward merged 2026-11-24. Not pushed. |
| `feature/self-healing-workspace` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/folderless-mode` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/cross-project-intelligence` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/research-librarian` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/knowledge-extraction` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/semantic-version-tracking` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/forgotten-asset-recovery` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/local-rag` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/system-care` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/embeddings-hybrid-search` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/d1-encryption-and-vec` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/context-modules-step7` | Merged (ff-only) into `master` on 2026-11-24. Not pushed. |
| `feature/put-back-together-and-learning` | Merged (ff-only) into `master` on 2026-11-23. Not pushed. |
| `feature/sets-and-photo-subjects` | Merged earlier into `master` (`43fc8bd`). Not pushed. |
| `feature/project-plans` | Tip matching pre-Plan-3 `master`. Not pushed. |
| `checkpoint/pre-stabilization-2026-11-23` | Annotated rollback tag at `084e3d5` (includes parked UI baseline `canonical/D1_Design_System_v1.0.md`, `canonical/D3_Critical_UX_Flows_v1.0.md`, `canonical/suite_reconciliation.md`, and `design-system/`). |

Verification on `master` (fast-forward from `feature/self-healing-workspace`):
- `cargo test --workspace`: **690 passed, 0 failed, 2 ignored** (`usn_refresh_on_system_temp`, `live_llama32_grounded_rag_answer`), including all **7** tests in `crates/aegis-core/tests/self_healing.rs` (`0.25s`), **227** tests in `aegis-core --lib` (incl. `healing::markdown::tests`, `healing::tags::tests`, `folderless::compiler::tests`, `folderless::engine::tests`, `cross_project::manifests::tests`, `cross_project::engine::tests`, `librarian::engine::tests`, `knowledge::parser::tests`, `db::vec::tests`, `versions::patterns::tests`, `assets::formats::tests`), **6** in `crates/aegis-core/tests/folderless_mode.rs` (`0.17s`), **6** in `crates/aegis-core/tests/cross_project.rs` (`0.14s`), **6** in `crates/aegis-core/tests/research_librarian.rs` (`0.15s`), **7** in `crates/aegis-core/tests/knowledge_extraction.rs`, **7** in `crates/aegis-core/tests/semantic_versions.rs`, **6** in `crates/aegis-core/tests/forgotten_assets.rs`, **5** in `crates/aegis-core/tests/local_rag.rs`, **4** unit tests in `crates/aegis-llm`, **6** in `crates/aegis-core/tests/system_care.rs`, **6** in `crates/aegis-core/tests/hybrid_search.rs`, **6** in `crates/aegis-core/tests/d1_encryption.rs` (with `R9` checking `["aegis-core", "aegis-embed", "aegis-llm"]`), **5** in `crates/aegis-embed` (`9.23s`, incl. live `nomic_gguf_live_semantic_check`), **11** in `aegis-desktop`, **19** in `crates/aegis-core/tests/drives.rs`, **7** in `crates/aegis-core/tests/context_modules.rs`, **7** in `crates/aegis-core/tests/core_invariants.rs`, **46** in `project_plans.rs`, **97** in `filing.rs`, **35** in `context.rs`, **6** in `reunite_and_learning.rs`, and **11** in `sets.rs`; **0** folders in `C:\.aegis-quarantine` after verification.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean (`0` warnings, `1.29s`).
- `cargo check -p aegis-desktop --features custom-protocol`: clean (`0.48s`).
- `pnpm build` (`tsc --noEmit && vite build`): clean (`2376` modules built in `909ms`).
- Prior Plan 5 live-DB dry run (`examples/filing_dry_run.rs` on a temporary read-only `VACUUM INTO` copy): **239** projects (`84` multi-location), **1** remaining ungathered design set (`MBIZO 1edited` on `D:`), **5** photo subjects, **18** software packages (`Microsoft Office 2024 Pro Plus LTSC...` whole at `102` files), **9** owner's folders, **4** resources, **52** series seasons, **2** old copies (`MSI Documents` `4256` files, `MSI Downloads` `7111` files), **249** project/set/old-copy plans in `18.8s` debug (including `New Volume` offline waits & same-drive plans), `"Old copy": 9747` kept whole on `New Volume` Fresh/Tidy, **1** `put back together` proposal (`ipadcover`), and weekly-note `projects` line `"84 projects have copies in 2–20 places. 1 group has files from earlier batches to put back together."`.

## 2. Done

### Programme (owner-approved order)

| # | Project | State |
|---|---|---|
| 1 | Drives & duplicates | Done, merged |
| 2 | Always-on engine: tray, scheduler, watches, reminders, weekly note | Done, merged |
| 3 | Auto-filing: blueprints (Tidy/Fresh), learn-then-take-over, naming, batches with undo | Done, merged 2026-11-22 |
| 3a | Context-aware filing, Plan 1: project recognition, filing never splits a project, Projects screen | Done, merged 2026-11-23 |
| 3b | Context-aware filing, Plan 2: project plans (move into `C:\Prj`, quarantine/versions/material, one batch per project with undo) | Done, merged 2026-11-23 (with stabilization pass) |
| 3c | Context-aware filing, Plan 3: sets, photo subjects, Design-vs-Software shelf separation, and inline UI reclassification | Done, merged 2026-11-23 |
| 3d | Context-aware filing, Plan 4: put back together (`file_history` reunion), `SameAs` learned aliases, weekly-note summary | Done, merged 2026-11-23 |
| 3e | Context-aware filing, Plan 5 (§16 step 7): Software packages, Owner's folders, Resources, Series, Old copies & external drive plans | Done, merged 2026-11-24 |
| 3f | D1 Encryption & `sqlite-vec` foundation (Plan 6A): SQLCipher whole-store, DPAPI `R2`, 24-word/hex recovery, `R7` sensitive views, `R9` guard | Done, merged 2026-11-24 |
| 3g | Local embeddings & Context-aware hybrid search (Plan 6B): pure-Rust `aegis-embed`, encrypted `vec_chunks`, RRF hybrid search, AC-gated `EmbedWatch` | Done, merged 2026-11-24 |
| 4 | System care (temp/caches/dead files, `R3` read-only apps/startup + leftover quarantine, SMART watch, trends, health score) | Done, merged 2026-11-24 |
| 3h | Local RAG Engine (Capability #3): pure-Rust `aegis-llm` (`Llama` / `Qwen2` GGUF + extractive), `core::rag` `K2`/`R7`, streaming `Ask Files` | Done, merged 2026-11-24 |
| 3i | Forgotten Asset Recovery (Capability #6 / 4.3): 80+ formats & header inspection, offline-drive & project matching, `K1`/`K2`/`K4`/`R7`, `ForgottenAssets` | Done, merged 2026-11-24 |
| 3j | Semantic Version Tracking (Capability #7 / 3.4): filename/`Versions\`/`file_history`/vector chains, LCS conceptual diff, optional local LLM summary, `K1`/`K2`/`K4`/`K5`/`R7`, `VersionTracking` | Done, merged 2026-11-24 |
| 3k | Automatic Knowledge Extraction (Capability #8 / 2.2): typed entity/transcript parsing, `K1` review inbox & incremental merge + undo, `K4` opt-in audio, `R7` masking, `KnowledgeBase` | Done, merged 2026-11-24 |
| 3l | Autonomous Research Librarian (Capability #9 / 2.3): disk/topic cross-referencing across `vec_chunks`/FTS5/Knowledge Graph, conceptual parallels (0 shared words), `R5`/`R11` disk-only, `ResearchLibrarian` | Done, merged 2026-11-24 |
| 3m | Cross-Project Intelligence (Capability #10 / 3.3): multi-project overlap, conflicting decisions, manifest version drift, 5-kind dependency mapping, `K1` triage, `R5`/`R7`/`R11`, `CrossProject` | Done, merged 2026-11-24 |
| 3n | AI Folderless Mode (Capability #11 / 2.1, reshaped `R6`): plain-language intents compiled into visible/editable/deletable filing rules, content/quarter/org/amount anchors, `K1` Simulator `Move` batches + undo, `K2`/`K4`/`R7`, `FolderlessMode` | Done, merged 2026-11-24 |
| 3o | Self-Healing Workspace (Capability #12 / 4.1): dynamic Markdown/wikilink repair across USN/scanner moves (`heal_move_log`) & `file_history`, YAML/inline tag consolidation & hex-color filtering, `folderless_rules` path audit, first-class `ActionType::PatchText` `K1` atomic patch + TOCTOU/rollback, `R1`/`R7`/`D1`, `SelfHealing` | Done, merged 2026-11-24 |
| 5 | Visual app (grows alongside the others) | Ongoing |

### Capability #12 (Self-Healing Workspace) in one paragraph

Repairs workspace link and taxonomy entropy across all indexed non-backup Markdown notes and automation rules without ever modifying files silently outside `K1` (`K1`–`K5`, `R1`, `R7`, `R8`, `R9`, `D1`). Migration `0016_self_healing.sql` adds SQLCipher-encrypted `heal_patch_payloads`, `heal_move_log`, and `heal_dismissals`; `db::files::upsert_discovered_file` logs external OS/USN renames into `heal_move_log` without polluting `file_history`, while `drives::registry::rebase_volume_paths` keeps paths rebased on drive re-lettering. First-class `ActionType::PatchText` integrates into `PolicyContext` (`RequireApproval("patch_text_requires_approval")`) and `db::actions` using atomic sibling temp writes (`MoveFileExW` on Windows), strict `blake3` TOCTOU guards on execute & rollback, FTS5/chunk refreshing, and `CopyBackKeepingBoth` post-heal edit protection. `crates/aegis-core/src/healing/` parses Markdown links, images, wikilinks, embeds, and reference definitions outside code spans (`markdown.rs`), extracts YAML frontmatter `tags:` + inline `#tags` to cluster casing mismatches, spelling variants (protecting numbered series like `#phase1`/`#phase2`), and `#A3E2F1` hex false positives (`tags.rs`), flags quarantined targets (`K2`), refuses auto-patching ambiguous matches, gates `confidential`/`secret` notes behind Windows CredUI unlock (`R7`), and audits stale `folderless_rules` paths alongside 8-point Doctor integrity checks into `src/screens/SelfHealing.tsx`.

### Capability #11 (AI Folderless Mode) in one paragraph

Delivers plain-language file routing compiled strictly into visible, editable, deletable filing rules without any opaque model disk access or unapproved moves (`K1`–`K5`, `R4`, `R6`, `R7`, `R9`, `D1`). Migration `0015_folderless_intents.sql` adds SQLCipher-encrypted `folderless_rules` inside `aegis.db`. `crates/aegis-core/src/folderless/compiler.rs` compiles natural-language rules (`"Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name"`, screenshots, contracts, installers, loose clutter) deterministically in pure Rust (`{quarter}`, `{year}`, `{org}`, `{kind}`, `{amount}` anchors) with optional off-lock `OnDemandLlm` refinement into transparent `CompiledIntentDraft` rules. `crates/aegis-core/src/folderless/engine.rs` evaluates rules against live non-backup catalog metadata and FTS5 chunks (`K2`/`K4`), shields `atomic_unit_id IS NOT NULL` code repos, gates `confidential`/`secret` documents behind CredUI unlock (`R7`), validates relative destinations against traversal/reserved names, resolves collisions safely, and stages every `ActionType::Move` through `PolicyContext` + `propose_batch` for Simulator preview and single-click `rollback_batch` (`K1`), exposed via 8 `folderless_*` IPC commands and `src/screens/FolderlessMode.tsx`.

### Capability #10 (Cross-Project Intelligence) in one paragraph

Eliminates project silos strictly from saved project folders, manifests, `vec_chunks` embeddings, and approved Knowledge Graph entities on disk without screen or input monitoring (`K1`–`K5`, `R4`, `R5`, `R7`, `R9`, `R11`, `D1`). Migration `0014_cross_project.sql` adds SQLCipher-encrypted `cross_project_insight_state` for reversible owner triage (`open`, `acknowledged`, `resolved`, `dismissed`). `crates/aegis-core/src/cross_project/manifests.rs` parses dependencies purely in Rust across `package.json`, `Cargo.toml`, `requirements.txt`, and `pyproject.toml`. `crates/aegis-core/src/cross_project/engine.rs` detects duplicate research/effort (`DuplicateEffort` via `vec_chunks` cosine, exact `blake3`, or lexical fallback while excluding raw manifest syntax noise), conflicting `Decision` entities across projects (`ConflictingDecision` across polarity verbs & mutually exclusive tech choices), major dependency version divergence (`VersionDivergence`), and 5 types of cross-functional dependency links (`SharedStakeholder`, `SharedDecision`, `CrossProjectAction`, `DocumentReference`, `SharedStack`), enforcing `K2`/`K4` exclusions, `R7` confidential/secret isolation, and off-lock `OnDemandLlm` executive brief synthesis into `src/screens/CrossProject.tsx`.

### Capability #9 (Autonomous Research Librarian) in one paragraph

Functions as a file- and project-grounded peripheral reading companion across the owner's encrypted catalog (`K1`–`K5`, `R4`, `R5`, `R7`, `R9`, `R11`, `D1`) without ever monitoring active windows, keystrokes, clipboard, or screen state (`R5`/`R11`). Migration `0013_research_librarian.sql` adds `librarian_pins` and `librarian_dismissals` inside SQLCipher `aegis.db`. `crates/aegis-core/src/librarian/engine.rs` populates a searchable shelf of indexed non-backup documents (`list_recent_research_docs`) and discovers related references for either a saved document on disk (`find_related_for_file`) or an explicit owner-entered topic/excerpt (`find_related_for_topic` with off-lock `OnDemandEmbedder`) by fusing three offline channels: encrypted `vec_chunks` cosine KNN via `load_file_vectors` + `vec_search_chunks` (flagging `ConceptualParallel` / `semantic_only` matches with zero shared keywords), FTS5 `chunks_fts` BM25 concepts, and shared approved Knowledge Graph entities/aliases from Capability #8 (`entities` + `relationships`), highlighting cross-project discoveries (`cross_project`), enforcing `K2`/`K4` exclusions, gating confidential/secret source and candidate documents under `R7`, and synthesizing optional cited `[1]..[N]` reading briefs via `OnDemandLlm` off the SQLite lock (`librarian_synthesize_brief`) into `src/screens/ResearchLibrarian.tsx`.

### Capability #8 (Automatic Knowledge Extraction) in one paragraph

Extracts structured intelligence (`Person`, `Organization`, `Project`, `Decision`, `ActionItem`) directly from local documents and `.vtt`/`.srt` meeting transcripts into an encrypted `K1` human-in-the-loop inbox (`K1`–`K5`, `R4`, `R7`, `R9`, `D1`). Migration `0012_knowledge_extraction.sql` adds `entity_aliases`, `knowledge_proposals`, and `knowledge_file_runs` alongside the existing B4 `entities` and `relationships` tables. `crates/aegis-core/src/knowledge/parser.rs` normalizes WebVTT/SRT cue files cleanly (handling speaker labels preceding decisions/actions), reuses `filing::orgs::ORGS` alongside corporate/project/email/task heuristics, and parses optional JSON or tagged output from `OnDemandLlm`. `crates/aegis-core/src/knowledge/store.rs` scans live non-backup catalog files idempotently (`knowledge_file_runs` watermarks), preserves dismissed owner decisions, incrementally merges approved entities via `entity_aliases` while storing snapshots for single-click `undo_approved_proposal`, links cross-entity provenance (`decided_in`, `assigned_in`, `mentioned_in`, `owns_action`, `belongs_to_project`) excluding quarantined/deleted files (`K2`), enforces `K4` opt-in for audio recordings, and gates confidential/secret provenance strictly under `R7`, surfaced through `src/screens/KnowledgeBase.tsx`.

### Capability #7 (Semantic Version Tracking) in one paragraph

Detects and organizes document and asset revision timelines across all non-backup catalogued volumes (`K1`–`K5`, `R7`, `R9`, `D1`, `FR-004.2`). Migration `0011_version_tracking.sql` adds `version_chain_prefs` (pins and chain dismissals) and `version_diff_summaries` inside SQLCipher `aegis.db`; `db::vec::load_file_vectors` reads mean-pooled, L2-normalized vectors from encrypted `vec_chunks`. `crates/aegis-core/src/versions/` normalizes filenames (`_v1`/`_v2`/`_FINAL`/dates/`Copy of` while preserving non-version sequences like `Lesson_01`), anchors project `Versions\` folders, links `file_history` renames/moves, clusters semantically similar non-sensitive drafts (`>= 92%` cosine) via Union-Find, flags exact `blake3` duplicate copies without ever auto-deleting semantic variants (`FR-004.2`), computes deterministic LCS hunks plus section/term badges (`diff_version_pair`, redacting locked `confidential`/`secret` files per `R7`), optionally summarizes non-sensitive text diffs via on-demand `OnDemandLlm` (`versions_summarize_diff`), and stages archival moves (`Versions\`) or duplicate quarantine through `propose_batch` (`K1`) into `src/screens/VersionTracking.tsx`.

### Capability #6 (Forgotten Asset Recovery) in one paragraph

Delivers non-destructive, in-place recovery of dormant or stranded creative and knowledge assets across all catalogued drives (`K1`, `K2`, `K3`, `K4`, `R7`, `R9`, `D1`). Migration `0010_forgotten_assets.sql` adds `asset_meta` and `asset_dismissals` inside SQLCipher `aegis.db`; `crates/aegis-core/src/assets/formats.rs` classifies 80+ extensions into 8 families (`DesignSource`, `RawPhoto`, `Image`, `ThreeD`, `Video`, `Audio`, `Typography`, `Document`) and parses visual headers (`PNG`, `JPEG`, `GIF`, `BMP`, `WebP`, `PSD/PSB`, `SVG` dimensions, aspect ratio, and dominant hex color) purely in Rust. `crates/aegis-core/src/assets/recovery.rs` surfaces assets buried in clutter (`Downloads`, `Desktop`, `OldCopy`), stored on unplugged drives (`OfflineDrive`, tracking `only_copy` across non-backup drives via `blake3`), or matching active projects/design sets (`StrandedFromCollection`), enforcing `K4` backup exclusion, `K2` quarantine exclusion, `R7` sensitivity masking (preventing chunk-text query leaks while locked), and staging every move through `propose_asset_recovery` -> `build_policy_context` -> `propose_batch` (`K1`) into `src/screens/ForgottenAssets.tsx`.

### Capability #3 (Local RAG Engine) in one paragraph

Adds 100% offline CPU document Q&A (`Ask Files`) over the SQLCipher-encrypted hybrid index (`K1`, `K2`, `K3`, `K5`, `R4`, `R7`, `R9`). `crates/aegis-llm` discovers local GGUF chat models recursively across `%USERPROFILE%\.ollama\models\manifests\` (`registry.ollama.ai` and `hf.co`) plus standalone GGUFs (recommending models `<= 4,500 MB`, defaulting to `1.9 GB` `llama3.2` when present with a `0 MB` `builtin:extractive` fallback), implements quantized pure-Rust `Llama` (interleaved `rope_i` + `rope_freqs.weight` + tied LM head) and `Qwen2` (split `rope` + QKV bias) inference with BPE/SentencePiece tokenizers and UTF-8 streaming, enforces `R9` zero-networking across `["aegis-core", "aegis-embed", "aegis-llm"]`, unloads `> 2.5 GB` models immediately after generation, and unloads compact models on `WindowEvent::Destroyed` or `60s` idle (`OnDemandLlm`). `crates/aegis-core/src/rag/mod.rs` strips question stopwords for natural-language FTS5/hybrid retrieval, enforces `K2` (never quarantined/deleted) and `R7` (excludes sensitive files unless CredUI-unlocked and requested, counting `excluded_sensitive_count` via SQLite IDs without loading chunks), and `src/screens/AskFiles.tsx` + `EngineSettings.tsx` deliver real-time token streaming and clickable file inspection.

### Project 4 (System care) in one paragraph

Delivers calm, non-destructive Windows System Care under every North Star guardrail (`K1`, `K2`, `K3`, `K5`, `R1`, `R3`, `R8`, `R9`). `crates/aegis-core/src/care/` scans aged temporary files, Windows CrashDumps/WER reports, disposable browser/dev caches (`Edge`, `Chrome`, `Brave`, `Vivaldi`, `Firefox`, `pip`, `npm`, `uv`, `NuGet`), active `cache_dirs`, Downloads janitor items, stale unowned leftover folders inside `%LOCALAPPDATA%`/`%APPDATA%`, and large stale untracked dead files (`DeadFiles`, >=100 MiB & >=180d outside projects/backups/atomic units) — proposing every removal exclusively into 30-day per-volume quarantine via strict `PolicyContext` allow-lists (`care_cleanup_roots`, `care_disposable_dirs`, `care_leftover_parents`, blocking protected vendor/Aegis dirs and auto-indexing uncatalogued files safely). Registry inspection (`care::apps`, `care::startup`) is strictly `KEY_READ`-only (`R3`): **Uninstall** invokes each app's own interactive `UninstallString` via backend registry lookup (`lookup_uninstall_command`), and Windows Apps / Startup Apps links open `ms-settings:appsfeatures` / `ms-settings:startupapps`. Non-admin Windows Storage CIM + `SmartHealthWatch` (`care::smart`, `care::watch`) monitors physical disk health without MFT/elevation (`R1`), `care::trends` computes 30d/90d volume and `root_samples` folder growth, `health_breakdown` scores the 5 storage-and-integrity dimensions (`storage`, `backup`, `duplication`, `caches`, `organization`), `janitor::installer_matches` fixes the deferred 3+ word false-match bug, and `src/screens/SystemCare.tsx` surfaces all 6 sub-tabs.

### Plan 6B in one paragraph

Wires 100% offline local semantic embeddings and hybrid retrieval (`K2`/`K3`/`K5`/`R4`/`R7`/`R9`/`D1`) inside the SQLCipher-encrypted `aegis.db` store without external runtimes or networking crates. `crates/aegis-embed` runs quantized GGUF (`nomic-bert` + standard `bert` with compact `QTensor`/`QMatMul` weights, RoPE/SwiGLU, and dual HF `##` / GGUF `\u{2581}` WordPiece tokenization) and Safetensors BERT models on CPU via pure-Rust `candle-core` + `candle-nn` alongside a zero-overhead `384`-d lexical hash fallback, loading on demand and unloading after indexing, `60s` idle, or window close (`<= 350 MB` `K5` budget). `crates/aegis-core` adds `db::vec` (`vec_chunks` virtual tables via `sqlite-vec` `0.1.9`, BLAKE3 change-hashed sync queue, chunk outbox retirement, and orphan pruning repaired by Doctor check #4), `db::search` (`search_hybrid` with RRF `k = 60`, `Project`/`DesignSet` filtering and collection-name boost, drive online/offline badges, mandatory `K2` quarantine exclusion, and `R7` confidential/secret snippet masking), and `Watch::needs_ac()` gating background `EmbedWatch` passes to idle AC mains, exposed through Settings (`Semantic Search & Local Embeddings`) and Search (`Hybrid` / `Keyword` / `Semantic` modes).

### Plan 6A in one paragraph

Encrypts the entire SQLite catalog (`aegis.db`) at rest with SQLCipher AES-256 (`sqlcipher-src` 4.20.0 + `libtomcrypt` 1.18.2 via `libsqlite3-sys-sqlcipher`, zero Perl/OpenSSL dependency) and registers `sqlite-vec` `0.1.9` on every `SqliteStore` connection alongside migration `0009_vault_and_vec_meta.sql` (`chunk_embeddings_meta`). Startup (`open_or_migrate_encrypted` in `db::crypto`) generates or DPAPI-seals the 256-bit key (`aegis.key.dpapi`, `R2`) for unattended engine/tray launches, performs verified one-time `sqlcipher_export` migration of legacy plaintext databases (`R8` temp cleanup), and locks behind an **Unlock Aegis Vault** screen if `aegis.key.dpapi` is missing on another account/PC until unlocked with the offline 24-word BIP-39 or 64-hex recovery key (`aegis.vault.json`). Sensitive files (`confidential`/`secret`, `R7`) remain searchable while masking snippets/chunk previews unless unlocked for a timed 15-minute session via Windows CredUI (`CredUIPromptForWindowsCredentialsW`), with full Settings (`Security & Encryption`) + Search UI controls, Doctor cipher/`sqlite-vec` checks, and an automated `Cargo.lock` guard (`R9`) blocking networking crates in `aegis-core` and `aegis-embed`.

### Plan 5 in one paragraph

Adds the five remaining Step 7 context modules plus safe same-volume external-drive project planning: **Software packages** (`context::software`) keep programs, drivers, and composite installers whole (`Microsoft Office 2024`, `DDU`, `Armoury_Crate_Uninstall_Tool`, `drivers`; routing in Fresh to `Software\Tools` or `Software\Installers`), **Owner's folders** (`context::owners`) protect purpose-named folders (`CVs`, `Ass. Bursar`, `Finance Manager`, `PhD`, `PM`, `.db`-pinned `Last30Days`/`DocAtlas Notes`) into their life areas, **Resources** (`context::resources`) protect skills/UI kits/templates into `Documents\Resources`, **Series** (`context::series`) pair episodic videos and subtitles into `Videos\Shows\<Show>\Season <n>`, and **Old copies** (`context::old_copies`) claim top-level backup trees (`D:\MSIDocuments`, `D:\MSIDownloads`) ahead of project discovery so inner repos don't claim the whole backup, proposing same-volume identical copies for BLAKE3-verified 30-day quarantine and others (plus cross-volume copies) for `StepKind::ToArchive`. External-drive plans run per drive (`offline_dirs`, `projects_homes`, and prefetched `summaries` in `Planner` keeping 249 plans fast offline) and set `waits_for` when unplugged without ever quarantining against another drive.

### Plan 4 in one paragraph

Earlier `filing:*` / `filing-auto:*` moves in `file_history` that scattered files out of a now-recognised project root, `DesignSet`, or `PhotoSubject` are surfaced per collection and volume as atomic **"Put back together"** proposals (`ReunitePlan` in `context::reunite`), tracing prior `MoveHome` directory moves, prioritizing `Project > DesignSet > PhotoSubject`, suffixing collisions `(2)` rather than overwriting, citing the split batch(es), updating `collection_members` / `LearnedSubject`, and supporting one-click undo (`undo_reunite` and Recent batches). Explicit `"Same project…"` answers (`Answer::SameAs`) teach normalized-name aliases (`learned_aliases` in `context::relate`) that match future similarly named folders while chronological `NotSameAs` vetoes/revokes them, and the weekly note (`WeeklyNote.projects` with `#[serde(default)]`) summarizes multi-location projects and pending reunite groups with a direct Review link to Projects.

Loose design and print files across formats and folders (`SmoothLite`, `Back Cement`) are recognised as `DesignSet` design projects in `collection_members` (migration `0008_sets.sql`, kept separate from `collection_locations`), protected in both `Fresh` and `Tidy` Filing (and retired from stale batches), and gathered into `C:\Prj\Design\<Name>` via `StepKind::GatherFiles` with collision suffixing, TOCTOU digest checks, immediate destination protection, and one-click undo. Dedicated Design shelf separation (`kind_home` + `Ctx::is_kind_shelf`) keeps `Design` work at `C:\Prj\Design\<Name>` while software/code projects stay flat at `C:\Prj\<Name>` (leaving nested `<Software>\Design` folders intact), `Answer::SetKind` / `DecisionKind::ProjectKind` allow one-click reclassification from the card, confirmation row, or compact row, and the Projects screen adds a kind filter pill bar with counts, icons, and Design styling. Loose photo subjects (`ipadcover`, `fragrances`) route under `Fresh` to `Photos\<year>\<subject>` while keeping named photo albums (`Pictures\Undated\mugs`) intact, and approved subjects (`DecisionKind::LearnedSubject`) catch single future arrivals. `src-tauri/Cargo.toml` also enables `custom-protocol = ["tauri/custom-protocol"]` so standalone desktop release builds embed `dist/` rather than pointing at `localhost:1420`.

### Plan 2 in one paragraph

Every project gets a plan per drive. The main copy moves whole into `C:\Prj`:
- identical and older copies go to quarantine, once every file is proven in the main copy by BLAKE3 (otherwise the whole copy is kept as a version);
- copies changed on both sides go to `Versions\<date> <origin>`;
- other material goes to `Docs`, `Design` or `Material`.

One approval runs it as one batch, with one undo. Safety rules from the owner's real data:
- nothing inside another project moves;
- Python environments never move;
- other drives get no plans yet;
- only homes inside a projects folder move in;
- copies changed in the last 30 days stay.

It was rehearsed on copies of the real forexTrader and DrivingSchool (117,044 files): undo restored every file exactly.

### Documents

- Specs: `docs/superpowers/specs/`. The context-aware filing design is `2026-11-22-context-aware-filing-design.md`.
- Plans: `docs/superpowers/plans/`. Plan 2 has "as built" notes and rulings per task.
- North star and binding rules for every future design: `docs/superpowers/specs/2026-11-23-local-intelligence-north-star.md`.

## 3. Waiting on the owner & recent decisions

**Decided 2026-11-23:**
- **Owner Decision D1 approved:** SQLCipher whole-store encryption + `sqlite-vec` inside the encrypted SQLite store (keeping vectors encrypted at rest within the 350 MB K5 budget) + DPAPI/TPM-sealed engine key (`R2`, Windows Hello only for sensitive views) + mandatory offline 24-word/hex recovery key; lands before embeddings and hybrid search. Owner also confirmed keeping all north-star exclusions and reshapings (`R1–R11`, explicitly including `R2` key sealing and `R6` AI Folderless Mode).
- **Orphaned quarantine folders removed:** Deleted all `57` pre-existing orphaned synthetic test folders from `C:\.aegis-quarantine` after confirming `quarantine_entries = 0` in the live database (`0` remaining).

**Still waiting on the owner:**
1. Choose a new app name (another "eagis" / Aegis app already exists).
2. Next active roadmap sequence: **Capability #3 (Local RAG engine)** (built on merged Plan 6B hybrid search) or **owner live-PC checks / Project 5 UI polish**, per owner choice.

## 4. What remains

### Context-aware filing (spec §16 build order)

| Step | What | State |
|---|---|---|
| 1–2 | Protection, Projects list | Done (Plan 1) |
| 3 | Project plans | Done (Plan 2) |
| 4 | Sets, photo subjects, Design-vs-Software shelf separation, and UI kind filtering | Done, merged 2026-11-23 (Plan 3) |
| 5–6 | Put back together (`file_history` reunion), `SameAs` learned name aliases, and weekly-note projects summary line | Done, merged 2026-11-23 (Plan 4) |
| 7 | Other modules: Software packages, Owner's folders, Old copies (drive D: & external-drive plans), Resources, Series | Done, merged 2026-11-24 (Plan 5) |

### Programme

- **Owner checks that need the installed build:**
  - `pnpm tauri build` and install;
  - notifications under "Aegis Guardian" open the right screen;
  - Ctrl+Shift+Space opens Search;
  - starts in the tray after a reboot;
  - at most 350 MB in the background.
- **Filing:**
  - dry run with drive D: plugged in (deferred item 14);
  - try Filing on the PC with the Fresh structure.
- **Phase 1 numbers with D: re-plugged:**
  - catalog time and fingerprint time;
  - file count and duplicate groups;
  - refresh mode and time.

### Vision (north-star order, each with its own short design)

1. ~~Embeddings + hybrid search (FTS5 + vectors, after D1).~~ **Done** (Plan 6A & Plan 6B merged 2026-11-24).
2. Local RAG over it (models on demand only) — **next vision capability**.
3. ~~Dead files and workspace health score.~~ **Done** (merged 2026-11-24 in Project 4 System care).
4. Forgotten asset recovery.
5. Semantic version tracking (builds on the `Versions` folders).
6. Knowledge extraction.
7. Research librarian and cross-project intelligence (work from files on disk, not the screen).
8. Folderless mode: plain-language requests become visible filing rules.
9. Self-healing workspace.
10. Chief Digital Officer agent and digital estate (no destructive dead-man's switch).

## 5. Known issues and follow-ups

**Noticed 2026-11-23 (and stabilization pass):**
- ~~The owner's running `aegis-desktop` used about 2 cores continuously and about 1 GB of memory (budget: 350 MB).~~ **Fixed (`b9899b2`, `6a4afde`):** capped SQLite `cache_size` to `-16384` (16 MB/conn) + `shrink_memory()` after heavy batches, filtered `is_live_excluded_path` and directory `Modified` events in `engine_host`, drained up to 2,000 queued FS events per wakeup, and removed per-file `canonicalize` syscalls during expansion while handling Windows case-only folder renames. *(Owner live runtime check on installed build remains under §4.)*
- ~~Older test files (not the plan tests) still write into the real `C:\.aegis-quarantine`.~~ **Fixed (`6a4afde`):** fixed `ensure_volume_for_path` so `ON CONFLICT DO UPDATE` preserves `quarantine_dir`, added `guard_test_quarantine()` refusing non-temp quarantine for in-memory test stores, and isolated `downloads` and `project_plans` tests (`57` before / `57` after full workspace suite).
- ~~Two timing tests are sensitive to machine load.~~ Both `recognise_is_fast_on_a_big_catalog` and `migration_speed_and_fts_sync` now pass in full workspace runs after the background churn fix.
- **Fixed (`c63e967`):** six Plan 2 bugs — blocked occupant guard, `MoveAside` collision suffixing, K1 `expected_digest` check on `context_run_plan`, `"location"` inside `propose` evidence, honest `rollback_batch` conflict reporting, `plan::undo` `move_home` ordering, and `volume_id`-scoped `rebase_children`/folder quarantine/restore with `outbox_events` and `cache_dirs`/`atomic_units` cleanup.
- Project plans' "files moved" counts catalogued files only (`node_modules` and `.venv` contents aren't catalogued).
- ~~Copies of a project on a drive without its home have no relationship to each other; they wait for the Old copies module.~~ **Resolved (Plan 5, 2026-11-24):** external-drive `plan_on` compares same-drive copies against that drive's own best copy (`same_vol_rel`, never quarantining against another drive's home) and `OldCopy` compares backup trees (`D:\MSIDocuments`, `D:\MSIDownloads`) against live counterparts via `StepKind::ToArchive` / same-drive quarantine.

**Filing follow-ups:**
- Search filters only apply to a typed query, and changing a filter needs another press of Search.
- The filing watch only notices arrivals on scopes that have had a batch approved.
- Code exercise folders inside course folders count as small projects, so those courses are not moved to `Videos\Courses`.

**Deferred from Project 2:**
- The pending-navigation slot can land on an old screen.
- The hard-coded shortcut label.
- Saving an invalid shortcut unregisters the working one.
- The `mark_interrupted_jobs` race at startup.
- ~~The installer-name first-word match.~~ **Resolved (Project 4, 2026-11-24):** `installer_matches` now requires the second meaningful word to match whenever both the installer and installed app have two or more words.
- Rename events counted twice by the tripwire.
- Copy fixes: the pause day, the janitor title, where only-copy files are.
- Halt cards "(0 of N)".
- A busy error aborts a tick.
- The 24 h grace clock resets on restart.
- Action Center click after the popup has gone.

**Deferred from Project 1:**
- **Resolved (`6a4afde`):** `.aegis-partial` on-disk BLAKE3 re-hash before rename, `path_hash` `volume_id` scoping (`retire_copy`, `resolve_policy_target_meta`, `copy_survivor_for`), duplicate keeper radio `keeper_index` wiring end-to-end, and extended `rebase_volume_paths` (`collection_locations`, `file_history`, `scan_jobs`, `root_samples`, `settings` roots, pending `actions`).
- **Still open:**
  - A `backup_jobs` row stays `running` after an error.
  - The first top-up isn't shown for approval.
  - There are two verified-copy implementations.
  - A large USN replay runs in one transaction.

## 6. Owner decisions on record (keep)

- Learn-then-take-over autonomy. Deletes always ask and go to the 30-day quarantine.
- One Backup drive (mirror, 90-day window). The Backup drive is never read.
- Reminders, not interruptions. The "I didn't know I needed this" factor.
- All projects, software or not, live in `C:\Prj`. A git repo may move whole when its plan is approved.
- Projects are one module of a whole-PC context layer, not the whole system.
- Default licence AGPL-3.0.
- Never commit the owner's `canonical/` documents.
