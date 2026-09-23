/**
 * Rust bridge glue for the `bx-api-perform` channels whose napi backends
 * exist (`docs/rust-napi-bridge.md` §applications + §manifest, the w01
 * `station-bridge` crate). Loaded lazily and only when
 * `STATION_RUST_BRIDGE=1`; the default saga/action dispatch in `worker.ts`
 * is unchanged.
 *
 * The addon is a cdylib built as `station_bridge.dll` (Windows) /
 * `libstation_bridge.so` (Linux) / `libstation_bridge.dylib` (macOS) under
 * `rust/target/{release,debug}`. Node's `require` only picks up `.node`, so
 * try that first, then the platform fallback names. In packaged builds
 * electron-builder's smartUnpack unpacks the whole package holding the
 * `.node` (docs/rust-packaging-notes.md §Why smartUnpack covers a napi
 * addon), and Electron's asar shim rewrites the require path — nothing to
 * configure here.
 */

// channel -> napi export; exactly the perform channels whose backends exist.
// Third element: payload keys forwarded as positional args (same order as
// the napi signature), or null to pass the whole payload as one argument.
const BRIDGED_CHANNELS = [
  ['SearchApplication', 'searchApplications', ['query']],
  ['GetMostPopularApplication', 'getMostPopularApplications', []],
  ['GetAllCategories', 'getAllCategories', []],
  ['GetApplicationsByCategory', 'getApplicationsByCategory', []],
  ['GetManifestByURL', 'getManifestByUrl', ['manifestURL']],
  ['GetPrivateApplications', 'getPrivateApplications', []],
  ['InstallApplication', 'installApplication', ['manifestURL', 'context', 'inBackground']],
  ['UninstallApplication', 'uninstallApplication', ['applicationId']],
  ['RequestPrivateApplication', 'requestPrivateApplication', null],
] as const;

export type BridgedChannel = typeof BRIDGED_CHANNELS[number][0];

let bridge: any;

/** `true` when the rust path is enabled AND the addon loaded. */
export const isRustBridgeEnabled = () => bridge !== undefined;

/**
 * Load the addon. Returns the export map, or `undefined` when no candidate
 * resolves (bridge off — worker keeps the saga dispatch).
 *
 * Node's `require` only dlopens the `.node` extension; cargo's cdylib output
 * is `station_bridge.dll`/`libstation_bridge.so`, so in dev the dll is
 * copied next to itself as `station_bridge.node` and that is required.
 * Packaged builds ship `station-bridge/station_bridge.node` (electron-builder
 * smartUnpack unpacks the containing package, docs/rust-packaging-notes.md).
 */
export const loadRustBridge = () => {
  if (bridge !== undefined) return bridge;

  // packaged layout first, then dev profiles relative to the app cwd
  const candidates = [
    'station-bridge/station_bridge.node',
    'rust/target/release/station_bridge.node',
    'rust/target/debug/station_bridge.node',
  ];
  const cdylibFallbacks = [
    ['rust/target/release/station_bridge.dll', 'rust/target/release/station_bridge.node'],
    ['rust/target/debug/station_bridge.dll', 'rust/target/debug/station_bridge.node'],
  ];

  for (const candidate of candidates) {
    try {
      // webpackIgnore: resolved at runtime, not bundled
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      bridge = require(/* webpackIgnore: true */ candidate);
      return bridge;
    } catch (e) {
      // try next candidate
    }
  }

  for (const [dll, node] of cdylibFallbacks) {
    try {
      const fs = require('fs');
      const path = require('path');
      const from = path.resolve(process.cwd(), dll);
      const to = path.resolve(process.cwd(), node);
      fs.copyFileSync(from, to);
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      bridge = require(/* webpackIgnore: true */ to);
      return bridge;
    } catch (e) {
      // try next fallback
    }
  }

  bridge = undefined;
  return undefined;
};

/**
 * `worker.ts` `callAction` rust path. Returns the response envelope (or
 * `undefined` for action-shaped channels) when `channel` is bridged,
 * `null` when the channel is not bridged (caller falls back to dispatch).
 */
export const rustBridgeCallAction = (channel: string, payload: any): Promise<any> | null => {
  if (bridge === undefined) return null;

  const entry = BRIDGED_CHANNELS.find(([c]) => c === channel);
  if (!entry) return null;

  const [, fnName, params] = entry;
  const fn = bridge[fnName];
  if (typeof fn !== 'function') return null;

  const args = params === null
    ? [payload]
    : params.map((p: string) => (payload ? payload[p] : undefined));
  return fn(...args);
};

/**
 * Selector-watch path (w03, bridge doc §theme/§identities + snooze row).
 * Channel -> [watchFn, unwatchFn, emitFn] on the addon; all three exist for
 * every channel so teardown and store taps share one table.
 */
const WATCHED_CHANNELS: Record<string, [string, string, string]> = {
  GetThemeColors: ['watchThemeColors', 'unwatchThemeColors', 'emitThemeColors'],
  GetAllIdentities: ['watchIdentities', 'unwatchIdentities', 'emitIdentities'],
  GetSnoozeDuration: ['watchSnoozeDuration', 'unwatchSnoozeDuration', 'emitSnoozeDuration'],
};

/**
 * Register `onValue` on the Rust-side hub for `channel`. The current value
 * (if one was already emitted) is delivered synchronously — same contract as
 * `subscribeStore`'s immediate first emission. Returns a teardown fn, or
 * `null` when the channel is not bridged / the addon is not loaded.
 */
export const rustBridgeSubscribe = (
  channel: string,
  onValue: (value: any) => void
): (() => void) | null => {
  if (bridge === undefined) return null;

  const entry = WATCHED_CHANNELS[channel];
  if (!entry) return null;

  const [watchFn, unwatchFn] = entry;
  if (typeof bridge[watchFn] !== 'function') return null;

  const id = bridge[watchFn](onValue);
  // Explicit worker-side teardown — the deliberate divergence from the
  // renderer-local ipcRenderer.off leak noted in the bridge doc §Transport.
  return () => { bridge[unwatchFn](id); };
};

/**
 * Push a freshly computed selector value into the Rust hub, which fans it out
 * to every subscribed renderer. Returns true when handled.
 */
export const rustBridgeEmit = (channel: string, value: any): boolean => {
  if (bridge === undefined) return false;

  const entry = WATCHED_CHANNELS[channel];
  if (!entry) return false;

  const emitFn = bridge[entry[2]];
  if (typeof emitFn !== 'function') return false;

  emitFn(value);
  return true;
};
