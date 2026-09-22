# s15 — Packaging notes: `.node` smartUnpack verification + future `asarUnpack` entries

Notes slice for `s15-packaging-tauri` **as amended 2026-09-22** (see
`docs/rust-rewrite-plan.md` line 5 and
`docs/research/rust-rewrite-decisions.md` §Packaging): shell = napi-rs into the
existing Electron app, NOT Tauri. s15 therefore keeps electron-builder
packaging and adds `.node` smartUnpack verification. This slice is docs-only:
**no config change yet** — no napi addon exists in the tree to unpack
(`rust/crates/` has no `crate-type = ["cdylib"]` target producing `.node`, and
no `station-bridge` crate yet; the s14 wiring slice
`docs/rust-napi-bridge.md` will add it).

## Sources read (authoritative)

1. `packages/app/electron-builder.yml` — current config (the artifact this
   document annotates; line numbers cited below are for the file as of this
   slice).
2. `node_modules/app-builder-lib/out/asar/asarUtil.js` (v26.15.7 installed;
   `packages/app/package.json` pins `electron-builder ^26.15.3`) —
   `processFileSets`: `if (this.config.options.smartUnpack !== false)` then
   `detectUnpackedDirs(fileSet, unpackedPaths)` per file set.
3. `node_modules/app-builder-lib/out/asar/unpackDetector.js` —
   `isLibOrExe(file)` returns true for `.dll`, `.exe`, `.dylib`, `.so`,
   **`.node`**; `detectUnpackedDirs` adds `stat.moduleRootPath` (the whole
   package root) to the unpack set when any file in the module matches, or the
   module is `ffprobe-static`/`ffmpeg-static`, or an extensionless file is
   binary. Files under an unpacked dir are emitted outside the asar
   (`app.asar.unpacked/`).
4. `node_modules/app-builder-lib/out/util/NodeModuleCopyHelper.js` —
   `stat.moduleRootPath = destination` is set per copied module, so "unpacked"
   granularity is the **package**, not the single `.node` file.
5. `scripts/after-pack.js` — the only afterPack hook kept (AppImage wrapper;
   unrelated to asar).

## Current config, annotated (with line citations)

- **Line 6** `asar: true` — asar packing on.
- **Lines 7–10** — comment documenting why there is **no explicit
  `asarUnpack`**: an unpack pattern makes app-builder-lib resolve every
  packaged file against the app dir, which throws "must be under" for
  workspace deps symlinked outside `packages/app` (`@getstation/sdk`
  (`packages/app/package.json:46`, `workspace:*`), `appstore`
  (`packages/app/package.json:201`, `workspace:*`)). And: "electron-builder's
  smartUnpack already unpacks any package holding .node/.dll".
- **Line 12** `afterPack: '../../scripts/after-pack.js'` — Linux wrapper only;
  does not touch asar.
- **Lines 70–91** `files:` — `!**/*.map` and `!**/node_modules/electron/**`
  exclusions; static asset `from:`/`to:` mappings. Relevant only in that any
  future `.node` file must NOT be excluded here.

## Why smartUnpack covers a future napi addon (semantics)

A napi-rs addon is a native Node-API module: build output
`station_bridge.<abi>.node` (napi-rs CLI naming; `.node` suffix is what
matters). electron-builder's smartUnpack (on unless
`smartUnpack: false`, see asarUtil.js above) detects any packaged file ending
in `.node` via `isLibOrExe` and unpacks the **entire containing package
root** to `app.asar.unpacked/`. Electron's main-process `require()` of the
addon must resolve there: dlopen cannot load a shared library from inside an
asar, and Electron's asar shim transparently rewrites
`app.asar/x.node` → `app.asar.unpacked/x.node` on `require`. No
configuration is needed for this to work — it is the default behavior of
electron-builder 26 for any `node_modules/**/*.node`.

## Future `asarUnpack` entries (when the addon lands — s14 wiring slice)

Add to `packages/app/electron-builder.yml` only if smartUnpack is ever
disabled or verification fails:

```yaml
asarUnpack:
  - 'node_modules/station-bridge/**'   # package-root pattern, matches how
                                       # smartUnpack unpacks whole modules
  # narrow fallback if package-root regresses the "must be under" symlink
  # error documented at electron-builder.yml:7-10:
  # - 'node_modules/station-bridge/*.node'
```

Preconditions before adding:

1. The s14 wiring slice creates `rust/crates/station-bridge` (cdylib) and a
   JS wrapper package installed at `node_modules/station-bridge` — until
   then there is nothing to unpack and the config stays untouched (this
   slice).
2. Prefer the package-root glob `node_modules/station-bridge/**` over
   `**/*.node` repo-wide: `detectUnpackedDirs` unpacks whole module roots
   anyway, and a global pattern re-triggers the file-resolution walk that
   broke on workspace symlinks (electron-builder.yml:7-10).
3. Keep the addon's loader using plain `require('station-bridge')` from
   `packages/app` main (per `docs/rust-napi-bridge.md` "Planned shape") so
   the file lands in `node_modules/` of the packaged app and both smartUnpack
   and any explicit `asarUnpack` pattern see it.

## Verification procedure (for the wiring slice that adds the addon)

Manual build check, no new scripts in this slice:

1. `yarn workspace @getstation/app release` (or a debug
   `electron-builder --dir`) on each CI OS.
2. Confirm the addon package directory exists outside the archive:
   `release/<plat>-unpack/resources/app.asar.unpacked/node_modules/station-bridge/`
   containing the `.node` file(s).
3. Confirm inside-asar absence:
   `npx asar list release/<plat>-unpack/resources/app.asar | grep station-bridge`
   exits 1 (or lists only the JS wrapper if the package is split).
4. Boot check: packaged app starts and a `bxApi` call that touches a napi
   export succeeds (dlopen from inside asar would crash main at startup).

## Slice boundaries

- **This slice (s15-packaging-notes)**: this file only. No
  `electron-builder.yml` change, no crate changes, no CI changes (s16).
- **s14 wiring slice**: creates the addon; runs the verification above; adds
  `asarUnpack` entries only if step 2/3 fails.
- **s16-ci-rust**: `cargo build/clippy/test` job; unrelated to packaging.
