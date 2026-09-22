export const meta = {
  name: 'rust-slice-executor',
  description: 'Overnight executor: dependency-ordered Rust rewrite slices, maker+verifier per slice, merge on PASS',
  phases: [
    { title: 'Execute', detail: 'per slice: maker agent builds, verifier agent checks, merge on pass' },
  ],
}

const REPO = 'C:/Users/asd/Documents/desktop-app'
const PATH_SETUP = `export PATH="$HOME/.cargo/bin:$PATH". Run npx/ctx7 only from OUTSIDE the repo dir (repo devEngines needs node>=24, machine has 22).`

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

// Slice order respects depends_on from docs/rust-rewrite-plan.md
const SLICES = [
  { id: 's05-ts-dead-code', title: 'Delete dead TS: packages/app/src/api/websocket.ts + ci/tsc.sh', verify: 'rg -n "SafeWS|websocket" packages/app/src/api must exit 1; rg -n "TRAVIS_COMMIT_RANGE" . --glob "!node_modules" --glob "!.git" must exit 1; repo TS build not required (files unreferenced — maker must prove with rg before deleting)' },
  { id: 's02-sdk-core', title: 'SDK core in rust/crates/station-sdk: common base + facade + leaf consumers (storage, config, history, ipc, resources, session) as typed traits/state over tokio sync primitives. Port types/traits from packages/sdk/src (read it first).', verify: 'cargo test -p station-sdk && cargo clippy -p station-sdk -- -D warnings (in rust/)' },
  { id: 's03-sdk-streams', title: 'SDK stream consumers in station-sdk: search + activity push/query types over tokio-stream (mpsc/broadcast). Port from packages/sdk/src/search + activity.', verify: 'cargo test -p station-sdk && cargo clippy -p station-sdk -- -D warnings' },
  { id: 's04-sdk-tabs-react-surface', title: 'SDK tabs + react consumer trait surface in station-sdk: webview-coupled ops as trait methods, no impl. Port from packages/sdk/src/tabs + react.', verify: 'cargo test -p station-sdk && cargo clippy -p station-sdk -- -D warnings' },
  { id: 's07-manifest-model', title: 'Manifest model in rust/crates/manifest-registry: serde BxAppManifest structs + embed packages/app/manifests/definitions JSON via include_dir + manifestToMinimalApplication mapping.', verify: 'cargo test -p manifest-registry: assert parsed count == JSON definition file count + serde round-trip' },
  { id: 's08-manifest-search-private', title: 'Manifest search + private manifests in manifest-registry: nucleo-matcher fuzzy search + user-added custom manifests persisted to config dir. AUTHOR golden-query fixtures FIRST (none exist).', verify: 'cargo test -p manifest-registry (golden queries pass)' },
  { id: 's09-manifest-config-rules', title: 'Manifest config rules in manifest-registry: isConfigurationRequired (subdomain/GoogleAccount/on-premise), handlebars identity-email templating (handlebars crate), getChromeExtensionId. Port from packages/app/src/abstract-application/helpers.ts. AUTHOR golden cases FIRST.', verify: 'cargo test -p manifest-registry config_rules (golden cases pass)' },
  { id: 's10-appstore-schema-types', title: 'Appstore schema in rust/crates/appstore-schema: copy packages/appstore/api-schema.graphqls verbatim to schema/api.graphqls, serde Application/Category + UI-state types replacing Apollo local cache.', verify: 'diff packages/appstore/api-schema.graphqls rust/crates/appstore-schema/schema/api.graphqls (empty) && cargo test -p appstore-schema' },
  { id: 's06-activity-store', title: 'Activity store in rust/crates/activity: sqlx SQLite (sqlite::memory: in tests) schema + push/query/merge, frecency merge from packages/app/src/activity as pure query logic.', verify: 'cargo test -p activity && cargo clippy -p activity -- -D warnings' },
  { id: 's11a-appstore-read-commands', title: 'Read-only appstore service: search/popular/categories over manifest-registry, exposed as napi-rs-ready plain functions in a new lib (no napi attr wiring yet — that is s14 wiring). Place in rust/crates/appstore-schema or new rust/crates/appstore-service crate.', verify: 'cargo test -p <crate> && cargo clippy -p <crate> -- -D warnings' },
  { id: 's11b-appstore-write-commands', title: 'Install/uninstall/request-private orchestration + app-request state machine port (from packages/appstore/src/app-request/duck.ts) + delete packages/appstore/src/applications/mockedAllAppsTemp.ts consumers note.', verify: 'cargo test -p <crate> state machine golden flows + clippy clean' },
  { id: 's12-about-window-parity', title: 'About window parity: Electron about-window logic notes + Rust state module. Given napi shell decision this is: port packages/app/src/about-window state handling to a small Rust module; Electron window itself unchanged.', verify: 'cargo check --workspace && clippy clean' },
  { id: 's13-protocol-handlers-notes', title: 'Protocol handlers: document + port bx-protocol/station:// handler logic (packages/app/src/webui/webUIHandler.ts) to Rust functions callable via napi; Electron registration stays.', verify: 'cargo test on new module + clippy clean' },
  { id: 's14-bridge-notes', title: 'Frontend bridge plan: map window.bxApi surface to napi exports, write docs/rust-napi-bridge.md mapping each bxApi method to planned napi function. No TS edits (renderer migration is post-overnight human-gated work).', verify: 'doc exists, covers every window.bxApi method found by rg "bxApi" — count matches' },
  { id: 's15-packaging-notes', title: 'Packaging notes: electron-builder.yml smartUnpack/.node verification doc + asarUnpack entries for future napi addon. No config change yet (no addon exists).', verify: 'doc exists, cites current electron-builder.yml lines' },
  { id: 's16-ci-rust', title: 'CI: add GitHub Actions workflow .github/workflows/rust.yml — cargo fmt --check, clippy -D warnings, test, Swatinem/rust-cache, on push/PR touching rust/. Keep existing workflows untouched.', verify: 'actionlint if available, else YAML parse via node yaml or python; workflow file valid + paths filter correct' },
]

function mkPrompt(s) {
  return `You are the MAKER for slice ${s.id} in repo ${REPO} (work on branch rust/${s.id} — create from current main: git checkout main && git pull && git checkout -b rust/${s.id}).

Task: ${s.title}

Binding rules:
- Read the source TS before porting. Port semantics, not lines.
- ONE slice only: no edits outside its scope. Never touch files belonging to other slices.
- Match repo Rust style (existing crates in rust/crates/).
- Use ctx7 for any crate API question (run from OUTSIDE repo dir): npx ctx7@latest library "<crate>" "<topic>" then docs <id>.
- rg for search; codegraph explore available.
- ${PATH_SETUP}
- Dependencies: workspace already has tokio/serde/serde_json in [workspace.dependencies] (versionless). Add version pins when you first use a dep (pick current stable; ctx7 check if unsure). Crates reuse via version.workspace/edition.workspace.
- Write tests where the slice verification demands them.
- Commit everything on your branch with message "feat(rust): ${s.id}" + Co-Authored-By: Claude Code <noreply@anthropic.com>. Do NOT push, do NOT open PR, do NOT merge — orchestrator handles.

Then run the slice verification yourself and include output tail in verification_output:
${s.verify}

Return structured result. status=PASS only if your verification run passed.`
}

function vfPrompt(s) {
  return `You are the INDEPENDENT VERIFIER for slice ${s.id} in repo ${REPO}. You did NOT write this code. Be adversarial: your job is to find reasons to fail it.

Branch rust/${s.id} exists with maker's commit. Verify:
1. Run the slice verification yourself (${PATH_SETUP}):
${s.verify}
2. Check scope discipline: git diff main...rust/${s.id} --stat — no files outside slice scope.
3. Spot-check port fidelity: read 2-3 core source TS files the slice claims to port, compare semantics against the Rust.

Do NOT fix anything. Do NOT commit. Verdict PASS only if verification passes + scope clean + no semantic drift found.

Return: verdict, evidence (command outputs tail), problems list.`
}

phase('Execute')
const results = []
const failures = {}

for (const s of SLICES) {
  // dependency gates
  const deps = { 's03-sdk-streams': ['s02-sdk-core'], 's04-sdk-tabs-react-surface': ['s02-sdk-core'], 's06-activity-store': ['s02-sdk-core', 's03-sdk-streams'], 's08-manifest-search-private': ['s07-manifest-model'], 's09-manifest-config-rules': ['s07-manifest-model'], 's11a-appstore-read-commands': ['s06-activity-store', 's08-manifest-search-private', 's09-manifest-config-rules', 's10-appstore-schema-types'], 's11b-appstore-write-commands': ['s11a-appstore-read-commands'], 's14-bridge-notes': ['s11b-appstore-write-commands', 's13-protocol-handlers-notes'], 's15-packaging-notes': ['s12-about-window-parity', 's14-bridge-notes'], 's16-ci-rust': ['s15-packaging-notes'] }[s.id] || []
  const blocked = deps.filter(d => failures[d] && failures[d] >= 3)
  if (blocked.length) { log(`${s.id} SKIPPED — blocked by hard-failed ${blocked.join(',')}`); results.push({ id: s.id, outcome: 'SKIPPED-BLOCKED', blocked }); continue }

  let pass = false
  for (let round = 1; round <= 3 && !pass; round++) {
    const made = await agent(mkPrompt(s) + (round > 1 ? `\n\nRETRY ROUND ${round}: previous attempt failed. Failure feedback: ${JSON.stringify(results.find(r => r.id === s.id)?.lastFail || 'unknown — check branch state, git log, fix forward')}` : ''), { label: `make:${s.id}:r${round}`, schema: RESULT_SCHEMA, effort: 'max' })
    if (!made) { log(`${s.id} maker agent died round ${round}`); continue }
    const v = await agent(vfPrompt(s), { label: `verify:${s.id}:r${round}`, schema: VERDICT_SCHEMA, effort: 'max' })
    if (v && v.verdict === 'PASS') {
      pass = true
      // merge: orchestrator-side, via agent to keep git work in agents? No — merge is deterministic shell work; but workflow scripts cannot run shell. Use a tiny agent.
      const merged = await agent(`In ${REPO}: git checkout main && git merge --no-ff rust/${s.id} -m "merge ${s.id} (verified PASS)" && git push origin main && git branch -d rust/${s.id} && git push origin --delete rust/${s.id}. ${PATH_SETUP} Return the final line of git log --oneline -1 as summary.`, { label: `merge:${s.id}`, schema: { type: 'object', properties: { summary: { type: 'string' }, status: { type: 'string', enum: ['OK', 'ERR'] } }, required: ['summary', 'status'] }, effort: 'low' })
      results.push({ id: s.id, outcome: 'PASS', rounds: round, merged: merged?.status, summary: made.summary, verification: made.verification_output?.slice(0, 400) })
      log(`${s.id} PASS round ${round}, merged=${merged?.status}`)
    } else {
      results.find(r => r.id === s.id)?.lastFail || results.push({ id: s.id, outcome: 'FAIL', lastFail: v?.problems || made.failure_reason || 'verifier unavailable' })
      const rec = results.find(r => r.id === s.id); rec.lastFail = v?.problems || made.failure_reason || 'verifier unavailable'
      log(`${s.id} FAIL round ${round}: ${JSON.stringify((v?.problems || made.failure_reason || 'no verifier') + '').slice(0, 200)}`)
    }
  }
  if (!pass) { failures[s.id] = (failures[s.id] || 0) + 3; results.push({ id: s.id, outcome: 'HARD-FAIL' }); log(`${s.id} HARD-FAIL after 3 rounds — dependents will skip`) }
}

log(`DONE: ${results.filter(r => r.outcome === 'PASS').length}/${SLICES.length} slices merged`)
return { results, ts: 'post-run' }
