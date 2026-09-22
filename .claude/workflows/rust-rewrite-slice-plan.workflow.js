export const meta = {
  name: 'rust-rewrite-slice-plan',
  description: 'Inventory desktop-app monorepo, produce verified Rust migration slice plan',
  phases: [
    { title: 'Map', detail: 'parallel readers over packages → module inventory' },
    { title: 'Slice', detail: 'synthesize ordered migration slice plan' },
    { title: 'Verify', detail: 'adversarial completeness + order check' },
  ],
}

const REPO = 'C:/Users/asd/Documents/desktop-app'

const INVENTORY_SCHEMA = {
  type: 'object',
  properties: {
    package: { type: 'string' },
    modules: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          name: { type: 'string' },
          path: { type: 'string' },
          purpose: { type: 'string' },
          loc: { type: 'number' },
          deps_internal: { type: 'array', items: { type: 'string' } },
          deps_external: { type: 'array', items: { type: 'string' } },
          rust_crate_hint: { type: 'string' },
          rewrite_risk: { type: 'string', enum: ['low', 'medium', 'high'] },
        },
        required: ['name', 'path', 'purpose', 'loc', 'deps_internal', 'deps_external', 'rust_crate_hint', 'rewrite_risk'],
      },
    },
    electron_coupling: { type: 'string' },
    notes: { type: 'string' },
  },
  required: ['package', 'modules', 'electron_coupling', 'notes'],
}

const PLAN_SCHEMA = {
  type: 'object',
  properties: {
    slices: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          order: { type: 'number' },
          id: { type: 'string' },
          title: { type: 'string' },
          source_paths: { type: 'array', items: { type: 'string' } },
          target_crate: { type: 'string' },
          depends_on: { type: 'array', items: { type: 'string' } },
          effort: { type: 'string', enum: ['S', 'M', 'L'] },
          verification: { type: 'string' },
        },
        required: ['order', 'id', 'title', 'source_paths', 'target_crate', 'depends_on', 'effort', 'verification'],
      },
    },
    workspace_layout: { type: 'string' },
    open_questions: { type: 'array', items: { type: 'string' } },
  },
  required: ['slices', 'workspace_layout', 'open_questions'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  properties: {
    complete: { type: 'boolean' },
    order_correct: { type: 'boolean' },
    missing: { type: 'array', items: { type: 'string' } },
    order_violations: { type: 'array', items: { type: 'string' } },
    slice_too_big: { type: 'array', items: { type: 'string' } },
  },
  required: ['complete', 'order_correct', 'missing', 'order_violations', 'slice_too_big'],
}

const TOOLS = `Tools binding: use codegraph explore CLI (repo indexed, .codegraph/ exists) BEFORE rg for symbol questions; rg for text search; never grep. For any library/API doc question (Rust crates, Electron, Tauri, GraphQL) run npx ctx7@latest library <name> "<topic>" then npx ctx7@latest docs <id> "<topic>" — never answer from memory. Repo root: ${REPO}.`

phase('Map')
log('Reading 3 packages + root config in parallel')

const targets = [
  { key: 'app', prompt: `Inventory packages/app of the Electron desktop app at ${REPO}. Read package.json, webpack configs, src/ and top-level dirs (activity, api, app-store, about-window, abstract-application, manifests). Identify distinct modules: their path, purpose, LOC, internal deps, external deps, which Rust crate could hold this logic, rewrite risk. State how deeply the package couples to Electron APIs (main process, BrowserView/webview, IPC) vs pure logic that ports cleanly. ${TOOLS}` },
  { key: 'appstore', prompt: `Inventory packages/appstore at ${REPO}: GraphQL schema files (api-schema.graphqls, client-schema.graphqls, schema.json), handler.ts, src/. Map resolvers/handlers to modules with LOC, deps, Rust crate hint (e.g. async-graphql, juniper), risk. Note data layer (what DB/storage it talks to). ${TOOLS}` },
  { key: 'sdk', prompt: `Inventory packages/sdk at ${REPO}: src/, package.json, README. What API does this SDK expose, who consumes it (search packages/app for imports), LOC per module, deps, Rust crate hint, risk. ${TOOLS}` },
  { key: 'root', prompt: `Analyze root build/CI/config of ${REPO}: package.json workspaces, ci/, scripts/, electron-builder.yml in packages/app, dev_utils/. What does CI run, what does the release pipeline produce (platforms, artifacts), what build tooling exists. Output as package="root" with one module per build concern. electron_coupling = summary of how builds depend on Electron toolchain. ${TOOLS}` },
]

const inventories = (await parallel(targets.map(t => () =>
  agent(t.prompt, { label: `map:${t.key}`, phase: 'Map', schema: INVENTORY_SCHEMA, model: 'opus', effort: 'max' })
))).filter(Boolean)

log(`Mapped ${inventories.length}/4 targets, ${inventories.reduce((n, i) => n + i.modules.length, 0)} modules total`)

phase('Slice')
const plan = await agent(
  `You are planning a TypeScript Electron monorepo rewrite in Rust, following loop-engineering refactor discipline: many small slices, each slice one PR, verifier per slice, dependency-ordered.

Inventories from parallel readers (JSON):
${JSON.stringify(inventories, null, 1)}

Produce an ordered slice plan:
- Slice 1 MUST be the Rust cargo workspace scaffold (empty crates, cargo check green).
- Order: leaf/pure-logic modules first, Electron-coupled last. SDK and shared types before app code that consumes them.
- Each slice: source paths it eliminates, target crate, depends_on slice ids, effort S/M/L, verification command or check (must be mechanical: cargo check/test/clippy, graphql schema diff, etc).
- Prefer many small slices over few big ones. No slice may bundle two unrelated modules.
- workspace_layout: proposed Cargo workspace tree as plain text.
- open_questions: decisions needing the human (e.g. Tauri vs wry vs keep-Electron-shell-with-Rust-core).

${TOOLS}`,
  { label: 'slice:plan', phase: 'Slice', schema: PLAN_SCHEMA, model: 'opus', effort: 'max' }
)

log(`Plan: ${plan.slices.length} slices`)

phase('Verify')
const lenses = [
  { key: 'completeness', prompt: `Adversarially verify this Rust migration slice plan for COMPLETENESS. Every module in the inventories below must be covered by some slice's source_paths, or explicitly listed as dropped. Hunt for orphaned modules. Inventories: ${JSON.stringify(inventories.map(i => ({ p: i.package, m: i.modules.map(m => m.path) })))}. Plan: ${JSON.stringify(plan)}` },
  { key: 'order', prompt: `Adversarially verify this Rust migration slice plan's DEPENDENCY ORDER. depends_on must form a DAG; no slice may depend on a later slice; slices consuming SDK/shared types must come after those. Also check no slice is too big to be one PR (L effort with multiple unrelated source trees = flag it). Plan: ${JSON.stringify(plan)}` },
  { key: 'verifiability', prompt: `Adversarially verify this Rust migration slice plan's VERIFICATION STEPS. Each slice's verification must be mechanical and runnable (cargo check/test, schema diff, dead-code elimination proof). Flag any slice whose verification is vague ("works", "reviewed"). Plan: ${JSON.stringify(plan)}` },
]

const verdicts = (await parallel(lenses.map(l => () =>
  agent(l.prompt, { label: `verify:${l.key}`, phase: 'Verify', schema: VERDICT_SCHEMA, model: 'opus', effort: 'max' })
))).filter(Boolean)

const allClear = verdicts.every(v => v.complete && v.order_correct && !v.slice_too_big.length)
log(allClear ? 'All lenses pass' : `Lenses flagged: ${JSON.stringify(verdicts.filter(v => !v.complete || !v.order_correct || v.slice_too_big.length))}`)

return { plan, verdicts, allClear, inventories }
