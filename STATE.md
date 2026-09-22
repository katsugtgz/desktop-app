# Loop State — My Project

Last run: 2026-09-22 dryrun L1 triage (report-only)

## High Priority (loop is acting or waiting on human)

1. **[Rust Rewrite] Decompose monorepo into Rust migration slices.** 3 packages: `app` (Electron main+renderer, ~849 ts/tsx), `appstore` (GraphQL API), `sdk`. Workflow phase 1 = inventory + slice plan. Slice = one PR.
2. **[Rust Rewrite] Scaffold Rust workspace** (`Cargo.tom` workspace + crate per package) as first slice, verified by `cargo check`.

## Watch List

- Upstream Tests CI green (2026-08-15). One Release Candidate failure before. No runs on fork yet — first PR will validate.
- Fork issues disabled; upstream issues = noise source only.
- Repo `devEngines` requires node>=24; local v22. Run `npx` from outside repo dir.
- CodeGraph indexed (`.codegraph/` exists). Use before rg for symbol/call-path questions.
- `packages/app` webviews: Electron. Rust rewrite target stack decision (Tauri vs pure browser) pending — needs human input or spike slice.

## Recent Noise (ignored this run)

- Upstream `tslint` deprecations, old release workflow — irrelevant to Rust path.
- Mermaid/NSIS/Python files in language stats — build tooling only.

---
Run log: see `loop-run-log.md`
