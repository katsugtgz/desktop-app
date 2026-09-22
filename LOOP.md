# Loop Configuration — Minimal Triage (Claude Code)

## Active Loops

| Pattern | Cadence | Status | Command |
|---------|---------|--------|---------|
| Daily Triage | 1d | L1 report-only | `/loop 1d Run $loop-triage` |
| Rust Rewrite | per-slice | L2 maker→verifier | Workflow per slice, see `STATE.md` |

## Objective — Rust Rewrite

Rewrite this repo (fork of getstation/desktop-app) in Rust. One slice = one module = one PR. Queue lives in STATE.md (High Priority = next slices).
Path: [refactor path](https://github.com/cobusgreyling/loop-engineering/blob/main/docs/refactor.md) — decompose, never one big-bang PR.

## Must-Use Tools (binding, every loop run and every sub-agent)

- **`rg`** — code search. Never `grep`/`findstr`.
- **CodeGraph** — `.codegraph/` exists: use `codegraph explore` or MCP `codegraph_explore` BEFORE rg for symbol/call-path questions. No `.codegraph/`: run `codegraph index` first, then use it.
- **ctx7** — `npx ctx7@latest library <name> "<topic>"` then `npx ctx7@latest docs <id> "<topic>"` for any library/framework/API doc question (Rust crates, Electron, Tauri included). Never answer library questions from memory.
- **Model**: opus[1m] for all sub-agents. **Effort**: max. Never downgrade.

## Human Gates

- No auto-fix until L2 checklist complete
- All high-risk paths: human review required (see docs/safety.md denylist)

## Worktrees

- Use `isolation: worktree` when spawning implementer sub-agents (L2+).
- One worktree per fix attempt; discard after verifier REJECT.

## Connectors (MCP)

- MCP optional for L1 report-only loops.
- For L2+: GitHub MCP to read CI/issues; scope connectors to read + comment only until trusted.

## Budget

- Max sub-agent spawns per run: 0 (L1)
- Review STATE.md daily

## Links

- Pattern: [daily-triage](../../patterns/daily-triage.md)
- Checklist: [loop-design-checklist](../../docs/loop-design-checklist.md)