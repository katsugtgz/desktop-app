#!/usr/bin/env node
/**
 * Smoke: load the station-bridge napi addon from plain Node (v22; napi4
 * target so Electron's bundled Node works too) and exercise every bridged
 * perform channel end-to-end against a scratch private-manifests store.
 *
 * Usage: node scripts/rust-bridge-smoke.js [path-to-.node]
 * Exit 0 = pass, 1 = fail. No Electron needed.
 */
'use strict';

const path = require('path');
const os = require('os');
const fs = require('fs');

const fails = [];
const check = (name, cond, detail) => {
  if (cond) {
    console.log(`ok - ${name}`);
  } else {
    fails.push(name);
    console.error(`FAIL - ${name}${detail ? `: ${JSON.stringify(detail).slice(0, 400)}` : ''}`);
  }
};

// resolve the addon: explicit arg (any extension), or cargo's profiles.
// Node dlopens only `.node`; a cdylib (`.dll` / `lib*.so`) is copied next to
// itself as `<name>.node` and that copy is required.
const explicit = process.argv[2];
const repoRoot = path.resolve(__dirname, '..');
const profiles = process.platform === 'win32'
  ? ['release/station_bridge.dll', 'debug/station_bridge.dll']
  : ['release/libstation_bridge.so', 'debug/libstation_bridge.so'];

let bridge;
let loadedFrom;
if (explicit) {
  const abs = path.resolve(explicit);
  const dotNode = abs.replace(/\.(dll|so|dylib)$/i, '.node');
  try {
    if (dotNode !== abs) {
      try { fs.copyFileSync(abs, dotNode); } catch (e) { /* existing copy is fine */ }
      bridge = require(dotNode);
    } else {
      bridge = require(abs);
    }
    loadedFrom = abs;
  } catch (e) {
    console.error(`FAIL - addon load: ${e.message}`);
    process.exit(1);
  }
} else {
  outer:
  for (const profile of profiles) {
    const from = path.join(repoRoot, 'rust/target', profile);
    const to = from.replace(/\.(dll|so|dylib)$/i, '.node');
    try {
      try { fs.copyFileSync(from, to); } catch (e) { /* existing copy is fine */ }
      bridge = require(to);
      loadedFrom = from;
      break outer;
    } catch (e) {
      // next profile
    }
  }
}

if (!bridge) {
  console.error('FAIL - addon load: none of the candidates resolved:');
  candidates.forEach((c) => console.error(`  ${c}`));
  process.exit(1);
}
console.log(`ok - addon loaded from ${loadedFrom}`);

// scratch store so install/uninstall never touch the user's real one
const scratch = path.join(os.tmpdir(), `station-bridge-smoke-${process.pid}.json`);
try { fs.unlinkSync(scratch); } catch (e) { /* absent is fine */ }
bridge.setManifestsPathForTests(scratch);

const isObj = (v) => v !== null && typeof v === 'object';

// queries: { body: ... } envelope
const search = bridge.searchApplications('slack');
check('searchApplications envelope', isObj(search) && Array.isArray(search.body));
check('searchApplications hits Slack', JSON.stringify(search).includes('Slack'));

const popular = bridge.getMostPopularApplications();
check('getMostPopularApplications envelope', isObj(popular) && isObj(popular.body));
check('popular keys', ['creamOfTheCropApps', 'runnerUps', 'noteworthy'].every((k) => k in popular.body));

const categories = bridge.getAllCategories();
check('getAllCategories envelope', isObj(categories) && Array.isArray(categories.body));

const byCategory = bridge.getApplicationsByCategory();
check('getApplicationsByCategory envelope', isObj(byCategory) && isObj(byCategory.body));

const manifest = bridge.getManifestByUrl('station-manifest://14');
check('getManifestByUrl envelope', isObj(manifest) && manifest.body && manifest.body.id === '14');

const privateBefore = bridge.getPrivateApplications();
check('getPrivateApplications envelope', isObj(privateBefore) && Array.isArray(privateBefore.body));

// requestPrivate: whole payload (recipe) as one arg, preload field names
const created = bridge.requestPrivateApplication({
  name: 'Smoke App',
  themeColor: '#ff0000',
  bxIconURL: 'https://example.com/icon.png',
  startURL: 'https://example.com',
  scope: 'https://example.com',
});
check('requestPrivateApplication envelope', isObj(created) && isObj(created.body));
check('requestPrivate returns id + bxAppManifestURL', Boolean(created.body && created.body.id && created.body.bxAppManifestURL));

// install: payload keys forwarded positionally, { body: applicationId }
// (a fresh persistent id, like the addApplicationRequest saga's return)
const installed = bridge.installApplication('station-manifest://14', null, true);
check('installApplication envelope', isObj(installed) && typeof installed.body === 'string' && installed.body.length > 0);

// uninstall: action-shaped channel resolves null
let uninstalled = null;
let uninstallThrew = null;
try { uninstalled = bridge.uninstallApplication(installed.body); }
catch (e) { uninstallThrew = e; }
check('uninstallApplication returns null', uninstalled === null && uninstallThrew === null, uninstallThrew && uninstallThrew.message);

// missing-param validation survives the bridge
let threw = null;
try { bridge.requestPrivateApplication({ name: 'x' }); } catch (e) { threw = e; }
check('requestPrivateApplication missing param throws', Boolean(threw && /missing/.test(String(threw.message || threw))));

try { fs.unlinkSync(scratch); } catch (e) { /* temp dir cleanup is best-effort */ }

if (fails.length) {
  console.error(`\n${fails.length} check(s) failed`);
  process.exit(1);
}
console.log('\nrust-bridge smoke: all checks passed');
