# Loop Budget — desktop-app Rust rewrite

> Primary loop: **Rust Slice Executor** (overnight autonomous, human-authorized 2026-09-22)

## Daily limits

| Loop | Max runs/day | Max tokens/day | Max sub-agent spawns/run |
|------|--------------|----------------|--------------------------|
| Daily Triage | 2 | 100k | 0 (L1) / 2 (L2) |
| Rust Slice Executor | unlimited | **no cap (human waiver 2026-09-22: "runs no budget while i sleep")** | as needed (maker + verifier per slice) |

## On budget exceed

Not applicable to Rust Slice Executor until human revokes waiver. Daily Triage: pause, log, notify per template.

## Kill switch

- Command or issue label: `loop-pause-all`
- STATE.md flag: `[PAUSED]` line in High Priority
- Human only.

## Slice Executor rules (binding, from LOOP.md + refactor path)

1. One slice per iteration: branch `rust/<slice-id>` → maker subagent → independent verifier subagent → merge on PASS → log → next slice.
2. Verifier = fresh agent, never the maker session. Verifier runs the slice's mechanical verification from docs/rust-rewrite-plan.md.
3. FAIL: verifier fixes nothing. Re-dispatch maker with failure output. 3 failed rounds on same slice = stop, flag STATE.md, skip to next independent slice.
4. Respect depends_on. No cross-slice file edits.
5. Tools binding (LOOP.md): rg, codegraph, ctx7 (npx from OUTSIDE repo dir), opus model, max effort.
