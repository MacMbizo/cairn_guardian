## What this changes

<!-- One or two sentences. -->

## Checklist

- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `cargo test --workspace` passes
- [ ] `pnpm build` passes
- [ ] File changes still go through the batch and undo pipeline; no outright deletes; no network access added
- [ ] Only bindings with real content changes are committed
