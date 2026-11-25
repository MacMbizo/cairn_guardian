# Contributing to Cairn Guardian

Thank you for helping. Cairn handles people's files, so the bar for safety is high. Please read this page before opening a pull request.

## Ground rules

Every change must keep these rules. A pull request that breaks one will not be merged.

1. **Propose, approve, undo.** Changes to user files go through the batch pipeline (`crates/aegis-core/src/db/actions.rs`), where they can be simulated and rolled back. Nothing writes to user files directly.
2. **No outright deletes.** Removals go to the 30-day quarantine.
3. **Local only.** The engine, embedding and model crates must not depend on networking crates. The `r9_forbids_networking_crates_in_engine_and_models` test enforces this.
4. **Opt-in observation.** Any new data source beyond file metadata and contents is off by default.
5. **Respect sensitive files.** Confidential and secret files stay masked unless the user has unlocked them.

## Development setup

Follow the [Getting started](README.md#getting-started) steps in the README.

## Before you open a pull request

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm build
```

- Add tests for new behaviour. Tests that quarantine files must point the volume's `quarantine_dir` at their own temporary folder.
- Running the tests regenerates the TypeScript bindings in `src/bindings`. Commit only bindings whose content changed, not files where only the line endings changed.
- Schema changes go in a new numbered migration in `crates/aegis-core/migrations`. Never edit a migration that has already shipped.
- Write interface text in plain, calm language. Say what happened and what the user can do next.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/), for example `feat(backup): ...`, `fix(policy): ...` or `docs: ...`.

## Licence

By contributing, you agree that your contributions are licensed under the [AGPL-3.0-or-later](LICENSE).
