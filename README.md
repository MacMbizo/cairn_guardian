<div align="center">

# Cairn Guardian

**A calm, local-first guardian for the files on your Windows PC and drives.**

Cairn learns how your files are organised, keeps them tidy, backed up and searchable, and never changes anything without a plan you can see and undo.

[![CI](https://github.com/MacMbizo/cairn_guardian/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/MacMbizo/cairn_guardian/actions/workflows/ci.yml)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue.svg)](LICENSE)
[![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6?logo=windows&logoColor=white)](#requirements)
[![Rust 2024](https://img.shields.io/badge/Rust-2024%20edition-B7410E?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app/)
[![React 19](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)](https://react.dev/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5-3178C6?logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Local only](https://img.shields.io/badge/network-none%20%C2%B7%20no%20telemetry-2E7D32)](#privacy-and-safety)
[![Status: pre-release](https://img.shields.io/badge/status-pre--release-orange)](#project-status)

</div>

---

## Contents

- [Why Cairn](#why-cairn)
- [What it does](#what-it-does)
- [Privacy and safety](#privacy-and-safety)
- [Requirements](#requirements)
- [Getting started](#getting-started)
- [Architecture](#architecture)
- [Repository layout](#repository-layout)
- [Testing](#testing)
- [Project status](#project-status)
- [Contributing](#contributing)
- [Security](#security)
- [License](#license)

## Why Cairn

A cairn is a stack of stones that marks the path for whoever comes next. Cairn Guardian does the same for your files: it shows you where everything is, where it should go, and the way back if you change your mind.

Most clean-up tools delete first and apologise later. Cairn works the other way round:

- **It proposes, you approve, and everything can be undone.** Each change is part of a batch with a single Undo.
- **Nothing is deleted outright.** Removed files wait in a 30-day quarantine on the same drive.
- **Everything stays on your PC.** The engine and its AI models run locally and make no network calls.

## What it does

| Area | What you get |
|---|---|
| **Catalog and live tracking** | A fast catalog of every PC folder and external drive, kept current through the NTFS change journal without admin rights. Offline drives stay searchable. |
| **Projects and context** | Recognises projects, design sets, photo subjects, software packages, series and old copies, even when one project is spread over several drives, and plans how to bring each one together in one place. |
| **Filing** | Plain-language rules ("invoices go to Finance by quarter") with a dry-run preview, plus a learn-then-take-over mode for routine filing. |
| **Search and Ask** | Hybrid keyword and semantic search across the whole catalog, and *Ask Files*: answers drawn from your own documents with citations, from a local language model. |
| **Backup drive** | A plain-folder mirror on a drive you choose (`Cairn Backup`), with top-ups, a change guard, priority filling, and previous versions. |
| **Duplicates and space** | Exact and near duplicates across drives, space trends and forecasts, and a five-part workspace health score. |
| **System care** | Temporary files, crash dumps, caches, large dead files and uninstall leftovers, all proposed for quarantine. Apps are removed only through their own uninstallers. |
| **Knowledge** | Extracts people, organisations, decisions and action items from notes and transcripts into a reviewable knowledge base. |
| **Versions and assets** | Tracks versions of a document across names and folders with readable diffs, and finds forgotten creative assets on any drive. |
| **Research and cross-project insight** | A research librarian that surfaces related reading, plus cross-project findings such as duplicated effort, conflicting decisions and diverging dependency versions. |
| **Self-healing** | Repairs broken Markdown and wiki links after files move, and tidies tag variants. |
| **Data governance** | Finds exposed secrets and sensitive documents, applies reversible labels, keeps chosen folders away from AI features, and reports your privacy posture. |
| **Chief Digital Officer** | A weekly executive view across all of the above, with proposals you ratify, and undo, in one click. |
| **Digital estate** | Splits your recovery key among trustees with Shamir secret sharing, prepares sealed succession packets, and runs a check-in heartbeat that can release instructions but **never** deletes or locks data. |

## Privacy and safety

Cairn is built around a short list of binding rules, enforced in code and tests:

- **Local only.** No cloud services, no telemetry. The engine, embedding and language-model crates may not depend on any networking crate; a test fails the build if one appears.
- **Encrypted at rest.** The catalog is a SQLCipher (AES-256) database. Its key is sealed to your Windows account with DPAPI, and you keep an offline 24-word recovery phrase.
- **Sensitive views need you.** Files labelled confidential or secret stay masked until you confirm with Windows Hello or your PIN.
- **Propose, approve, undo.** No rule, model or agent writes to disk outside the batch pipeline. Every batch has one Undo.
- **Quarantine instead of delete.** Deletions go to a 30-day quarantine on the same drive. Nothing is shredded.
- **Opt-in observation.** Anything beyond file names and contents, such as audio, is off until you turn it on.
- **Light in the background.** With the window closed the engine stays within a small memory budget. Heavy work waits for idle time and, for the heaviest, for mains power.

## Requirements

- Windows 10 or 11 (x64)
- [Rust](https://rustup.rs/) stable, 1.88 or newer, with the MSVC toolchain and the Visual Studio C++ Build Tools (SQLCipher is compiled from source; Perl and OpenSSL are not needed)
- [Node.js](https://nodejs.org/) 22 or newer and [pnpm](https://pnpm.io/) 10 or newer
- Optional: a local [Ollama](https://ollama.com/) model folder or GGUF files for semantic search and *Ask Files* (for example `nomic-embed-text` and `llama3.2`). Cairn reads the model files directly and never contacts a server.

## Getting started

```powershell
git clone https://github.com/MacMbizo/cairn_guardian.git
cd cairn_guardian
pnpm install
```

**Run in development** (two terminals):

```powershell
pnpm dev                       # the interface, on http://localhost:1420
cargo run -p aegis-desktop     # the desktop app and engine
```

**Build a standalone release:**

```powershell
pnpm build
cargo build --release -p aegis-desktop --features custom-protocol
# -> target\release\cairn-guardian.exe
```

**Build an installer** (NSIS and MSI) with the Tauri CLI:

```powershell
pnpm dlx @tauri-apps/cli build
```

> **Note on names.** Internal crate names (`aegis-core`, `aegis-desktop`, and others) and some on-disk files (`aegis.db`, the `com.aegis.guardian` data folder) keep the project's original working name, so existing data and installs keep working. Everything you see in the app says Cairn.

## Architecture

```text
+-----------------------------------------------------------+
|  Interface: React 19 + TypeScript + Tailwind (src/)       |
|  Typed IPC client generated from Rust with ts-rs          |
+---------------------------+-------------------------------+
                            | Tauri 2 commands and events
+---------------------------v-------------------------------+
|  Desktop shell (src-tauri/)                               |
|  tray, single instance, autostart, notifications,         |
|  background watches, Windows credential prompts           |
+---------------------------+-------------------------------+
                            |
+---------------------------v-------------------------------+
|  Engine (crates/aegis-core)                               |
|  catalog, USN tracking, policy, batches and undo,         |
|  quarantine, backup, context, filing, search, RAG,        |
|  governance, healing, estate ...                          |
|  SQLCipher + FTS5 + sqlite-vec, all in one encrypted file |
+--------------+-----------------------------+--------------+
               |                             |
+--------------v--------------+ +------------v--------------+
| crates/aegis-embed          | | crates/aegis-llm          |
| local text embeddings       | | local GGUF chat models    |
| (candle, pure Rust, on CPU) | | (Llama, Qwen2), extractive|
|                             | | fallback                  |
+-----------------------------+ +---------------------------+
```

Models load on demand and unload when idle, so the background engine stays small.

## Repository layout

| Path | Contents |
|---|---|
| `crates/aegis-core` | The engine library, its SQL migrations and integration tests |
| `crates/aegis-embed` | Local embedding models and tokenizers |
| `crates/aegis-llm` | Local language-model inference |
| `crates/libsqlite3-sys-sqlcipher` | SQLCipher build for MSVC with no Perl or OpenSSL |
| `src-tauri` | Desktop shell and IPC commands |
| `src` | React interface; `src/bindings` is generated, do not edit by hand |
| `docs` | Progress report, specs and implementation plans |

## Testing

```powershell
cargo test --workspace                                  # about 770 tests
cargo clippy --workspace --all-targets -- -D warnings
pnpm build                                              # type-check and bundle the interface
```

Tests that need an NTFS change journal or a large local model are marked `#[ignore]` and run manually. Tests use private temporary drives and quarantine folders, so they never touch your real files.

## Project status

Cairn Guardian is **pre-release** (`0.1.0`). All fifteen planned capabilities are built and tested. The current phase covers installer testing on real PCs, Windows Hello and BitLocker checks, and visual polish. See [`docs/PROGRESS.md`](docs/PROGRESS.md) for the detailed progress report.

## Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first. Every change must keep the [safety rules](#privacy-and-safety).

## Security

Please report vulnerabilities privately as described in [SECURITY.md](SECURITY.md), not in a public issue.

## License

Copyright © 2026 MacDonald Mbizo.

Cairn Guardian is free software, licensed under the [GNU Affero General Public License v3.0 or later](LICENSE). You may use, study, share and modify it. If you distribute it, or offer a modified version to users over a network, you must make your source code available under the same licence.
