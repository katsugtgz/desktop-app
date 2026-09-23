export const meta = {
  name: 'rust-bridge-executor',
  description: 'Wire Rust core into Electron via napi: station-bridge crate, main-process glue, dead-type cleanup',
  phases: [
    { title: 'Execute', detail: 'per slice: maker agent builds, verifier agent checks, merge on pass' },
  ],
}

const REPO = 'C:/Users/asd/Documents/desktop-app'
const PATH_SETUP = `export PATH="$HOME/.cargo/bin:$PATH". Run npx/ctx7 only from OUTSIDE the repo dir. Node on machine is v22; Electron's bundled Node is separate — target napi4 (Node 10+), no node-gyp needed.`

const RESULT_SCHEMA = {
  type: 'object',
  properties: {
    status: { type: 'string', enum: ['PASS', 'FAIL'] },
    branch: { type: 'string' },
    committed: { type: 'boolean' },
    summary: { type: 'string' },
    verification_output: { type: 'string' },
    failure_reason: { type: 'string' },
  },
  required: ['status', 'branch', 'committed', 'summary', 'verification_output'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['PASS', 'FAIL'] },
    evidence: { type: 'string' },
    problems: { type: 'array', items: { type: 'string' } },
  },
  required: ['verdict', 'evidence', 'problems'],
}

const COMMON = `Context: repo ${REPO}, fork of getstation/desktop-app. Phase 1 done: rust/ workspace, 7 crates, 136+ tests green, CI green. Read docs/rust-napi-bridge.md FIRST — it is the authoritative mapping (every bxApi method to napi export). Binding rules: rg for search; ctx7 for crate API questions (from OUTSIDE repo dir): npx ctx7@latest library "napi-rs" "<topic>" / docs <id>; ${PATH_SETUP}; one slice scope only; commit "feat(bridge): <slice-id>" + Co-Authored-By: Claude Code <noreply@anthropic.com>; no push/PR/merge — orchestrator handles.`

const SLICES = [
  {
    id: 'w01-station-bridge-crate',
    title: `Create rust/crates/station-bridge: cdylib, napi = "3", napi-derive = "3", napi-build = "2" build-dep, build.rs calling napi_build::setup(). Export the perform-shaped applications/manifest functions per docs/rust-napi-bridge.md §applications + §manifest that have EXISTS backends: search_applications(query)->Value ({body: MinimalApplication[]}), get_most_popular_applications(), get_all_categories(), get_applications_by_category(), get_manifest_by_url(manifest_url), get_private_applications(), install_application(manifest_url, context, in_background), uninstall_application(application_id), request_private_application(recipe as Value). Service state: a global Mutex<Option<ApplicationService>> initialized from embedded definitions + user config dir (lazy, like manifest-registry PrivateStore). Return serde_json::Value via napi's serde-json feature (napi = { version = "3", features = ["serde-json"] }). Use #[napi(js_name = "camelCase")] matching bridge doc column. Write Rust unit/integration tests: each function returns the documented envelope shape (command-shaped raw, query-shaped {body}). Do NOT wire Electron yet.`,
    verify: 'cd rust && cargo test -p station-bridge && cargo clippy -p station-bridge -- -D warnings && cargo check --workspace (workspace must stay green)',
  },
  {
    id: 'w02-glue-worker-swap',
    title: `Electron main-process glue per bridge doc §Planned shape: thin TS module that loads station-bridge .node (require with platform fallback path), registers ipcMain handlers for bx-api-perform channels replacing the saga/action dispatch in packages/app/src/services/services/sdkv2/worker.ts, forwarding to napi functions and responding on bx-api-perform-response-\${channel}. Keep webview-preload.js and ALL renderer code untouched. Selector channels (GetThemeColors/GetAllIdentities/GetSnoozeDuration) and notification passthrough stay on existing Electron paths this slice — swap ONLY the perform channels whose backends exist (the 10 applications + 1 manifest from w01). worker.ts: add the new path alongside existing dispatch guarded by a feature flag (env STATION_RUST_BRIDGE=1) so default behavior unchanged. Confirm .node loading: electron-builder smartUnpack already handles .node (docs/rust-napi-bridge.md research). Also add node-side smoke script scripts/rust-bridge-smoke.js run via plain node (not Electron) asserting require of the built cdylib works and search_applications("slack") returns body array.`,
    verify: 'cd rust && cargo build -p station-bridge; node ../scripts/rust-bridge-smoke.js exits 0 with body-array log; rg -n "STATION_RUST_BRIDGE" packages/app/src/services/services/sdkv2/worker.ts hits; rg "window\\.bxApi" packages/appstore/src -c still >0 (renderer shape untouched); yarn not required if unavailable — then document manual build step instead and PASS only if smoke script ran',
  },
  {
    id: 'w03-watchers-dead-types',
    title: `Two parts. (1) Watchers per bridge doc §theme/§identities: napi ThreadsafeFunction exports watch_theme_colors(cb), watch_identities(cb)/unwatch_identities(cb), watch_snooze_duration(cb) in station-bridge — Rust side holds subscriber lists behind Mutex, emit current value immediately on subscribe; TS glue forwards webContents.send on the existing bx-api-subscribe-response channels. Wire into worker.ts selector path behind same STATION_RUST_BRIDGE flag. Include explicit unsubscribe (bridge doc notes TS leak — deliberate divergence). (2) Dead-type cleanup in packages/app/src/plugins/bxapi.d.ts ONLY types: remove user/services/Runtime/AuthorizationError/NoMethodError/SystemError declarations after rg-verifying zero runtime references (bridge doc §Not ported).`,
    verify: 'cargo test -p station-bridge && cargo clippy -p station-bridge -- -D warnings; rg -n "AuthorizationError|NoMethodError|SystemError" packages/app/src --glob "!*bxapi.d.ts" exits 1; rg -n "watch_theme_colors|watch_identities|watch_snooze" rust/crates/station-bridge/src hits; cargo check --workspace green',
  },
]

function mkPrompt(s) {
  return `You are the MAKER for slice ${s.id}. ${COMMON}

git checkout main && git pull && git checkout -b bridge/${s.id}

Task: ${s.title}

Then run this verification and include output tail:
${s.verify}

Return structured result. PASS only if verification passed.`
}

function vfPrompt(s) {
  return `You are the INDEPENDENT VERIFIER for slice ${s.id} in ${REPO}. You did NOT write this code. Be adversarial. Branch bridge/${s.id}.

1. Run verification yourself (${PATH_SETUP}):
${s.verify}
2. Scope: git diff main...bridge/${s.id} --stat — only slice-scope files.
3. Fidelity: read docs/rust-napi-bridge.md rows for this slice; confirm each napi export name/shape matches the doc (js_name casing, envelope {body} vs raw).
4. For w02 additionally: confirm default (flag off) code path byte-identical in behavior — diff worker.ts dispatch logic.

Do NOT fix or commit. PASS only if all green.

Return verdict, evidence tail, problems.`
}

phase('Execute')
const results = []
for (const s of SLICES) {
  let pass = false
  for (let round = 1; round <= 3 && !pass; round++) {
    const prev = results.find(r => r.id === s.id)
    const made = await agent(mkPrompt(s) + (round > 1 ? `\n\nRETRY ${round}. Previous failure: ${JSON.stringify(prev?.lastFail || 'unknown')}. Fix forward on the existing branch.` : ''), { label: `make:${s.id}:r${round}`, schema: RESULT_SCHEMA, effort: 'max' })
    if (!made) { log(`${s.id} maker died r${round}`); continue }
    const v = await agent(vfPrompt(s), { label: `verify:${s.id}:r${round}`, schema: VERDICT_SCHEMA, effort: 'max' })
    if (v && v.verdict === 'PASS') {
      pass = true
      const merged = await agent(`In ${REPO}: git checkout main && git merge --no-ff bridge/${s.id} -m "merge ${s.id} (verified PASS)" && git push origin main && git branch -d bridge/${s.id} && git push origin --delete bridge/${s.id}. ${PATH_SETUP} Return final git log --oneline -1 line as summary.`, { label: `merge:${s.id}`, schema: { type: 'object', properties: { summary: { type: 'string' }, status: { type: 'string', enum: ['OK', 'ERR'] } }, required: ['summary', 'status'] }, effort: 'low' })
      results.push({ id: s.id, outcome: 'PASS', rounds: round, merged: merged?.status, summary: made.summary })
      log(`${s.id} PASS r${round} merged=${merged?.status}`)
    } else {
      const rec = { id: s.id, outcome: 'FAIL', lastFail: v?.problems || made.failure_reason || 'verifier unavailable' }
      const ex = results.find(r => r.id === s.id)
      if (ex) { ex.lastFail = rec.lastFail } else results.push(rec)
      log(`${s.id} FAIL r${round}: ${String(JSON.stringify(v?.problems || made.failure_reason)).slice(0, 200)}`)
    }
  }
  if (!pass) { results.push({ id: s.id, outcome: 'HARD-FAIL' }); log(`${s.id} HARD-FAIL`) }
}
log(`DONE: ${results.filter(r => r.outcome === 'PASS').length}/${SLICES.length}`)
return { results }
