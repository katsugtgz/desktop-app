# Surface-hide probe — do Rune surfaces hide the live page, and does it come back? (2026-11-18)

**Verdict: yes — all four surfaces hide the page window, and returning shows the same page again.**
Two launches of the release binary with the seeded Gmail session produced one `WRY_WEBVIEW`
child (1136×756 at the content rect) and one Chromium browser widget
(`Chrome_WidgetWin_1`, "Sign in - Google Accounts"). Pressing the rail's App Store row flipped
`IsWindowVisible(WRY_WEBVIEW)` to **false**; pressing the Gmail row flipped it back **true** with
the same `WRY_WEBVIEW` HWND, the same Chromium widget HWND, title and rect. Settings at
`(32, height−80)` and the notification moon at `(32, height−32)` did the same; `WM_KEYDOWN`
`VK_ESCAPE` restored the page after both, and a second moon press restored it by toggle. No
second `WRY_WEBVIEW` or browser widget appeared at any point.

## Machine / session facts (read-only checks, 2026-11-18)

- Binary used as-is, **not rebuilt**: `rust/target/release/station-shell.exe`, **29,073,408 B**,
  mtime `2026-11-18 19:51:47`, SHA-256
  `086c7eeabc814892d7ecefca27dfdbbe4231ff5bf5a44a9d4b237e38105c13ab`.
- Session: `qwinsta` → `console  asd  1  Active`; `SESSIONNAME=Console` — interactive console
  session 1. OS `Windows-10-10.0.19045-SP0`, AMD64. WebView2 runtime 157.0.4341.46 (sole install).
- Window: outer 1216×839 (run 1 at `(156,156)`, run 2 at `(78,78)`), **client 1200×800**
  (`GetClientRect`). `WRY_WEBVIEW` occupies the content rect: 1136×756 at client `(64,44)`
  (screen `(228,231)` / `(150,153)` in the two runs).
- Rail geometry comes from `rail.rs` + rune's tokens: rail width **64 pt**, vertical padding
  `space::S3 = 12`, gap `space::S2 = 8`, row **40 pt**. So with the store first and Gmail second:
  store row centre client `(32, 32)`, Gmail `(32, 80)`; bottom-up, the notification row centre is
  `(32, height−12−20) = (32, 768)` and Settings `(32, height−12−20−40−8) = (32, 720)` for a
  800 pt-high client.
- What is under test: `docs/rust-rune-port.md` D8 and `rust/README.md` — a Rune-drawn surface
  (Settings, notification panel, App Store) hides the live WebView2 page rather than covering it,
  and returning shows it without re-creating it.

## What I ran

Two launches (19:57:29 and 19:58:31 local) with scratch session paths — never the user profile:

```json
{
  "version": 1,
  "activeApplication": "14",
  "installed": [
    { "application_id": "14", "manifest_url": "station-manifest://14", "in_dock": true }
  ],
  "dock": ["14"],
  "tabs": [
    { "tab_id": "tab-gmail", "application_id": "14", "url": "https://mail.google.com/", "home": true }
  ]
}
```

`STATION_SESSION_PATH=%TEMP%\surface-hide-probe-<ts>\session.json
rust\target\release\station-shell.exe`, stderr/stdout captured in the same scratch dir. The
window was waited for (class `Window Class`, title `Station`), then the page (visible
`WRY_WEBVIEW` plus a visible `Chrome_WidgetWin_1`, title "Sign in - Google Accounts"), then 2 s of
settle. Observation is read-only Python 3.13 + ctypes.

Presses were posted to the top-level window, not injected as hardware input:

```
WM_MOUSEMOVE  (0x0200) wParam=0        lParam=(y<<16)|x
WM_LBUTTONDOWN(0x0201) wParam=MK_LBUTTON
WM_LBUTTONUP  (0x0202) wParam=0
```

80 ms apart at the same coordinate; winit takes the click position from the `WM_MOUSEMOVE`
(`MouseInput` itself carries none), so the move is required. Escape was
`WM_KEYDOWN`/`WM_KEYUP` `VK_ESCAPE` (0x1B) to the same window. No focus or activation helper is
needed: run 1 called `SetForegroundWindow`, which did **not** take
(`GetForegroundWindow() != shell`), and its posted clicks still worked; run 2 sent no focus calls
at all and every step worked. (Run 1's extra click on the top bar at client `(600,20)` changed
nothing.)

Coordinates were computed from the rail tokens above; in both runs the **first** candidate
worked, so no fallback grid point was used. After every press the harness polled
`IsWindowVisible` on every `WRY_WEBVIEW` child and `EnumChildWindows` for the Chromium children,
for up to 3 s.

## Exact outputs

**Baseline** (both runs, page visible):

```
run 1: WRY_WEBVIEW hwnd=0x3D4F1490 visible=True rect=(228,231)-(1364,987)  1136×756
       Chrome_WidgetWin_1 hwnd=0x2B570B36 title="Sign in - Google Accounts" visible=True
run 2: WRY_WEBVIEW hwnd=0x3EFC1338 visible=True rect=(150,153)-(1286,909)  1136×756
       Chrome_WidgetWin_0 hwnd=0x64B138A
       Chrome_WidgetWin_1 hwnd=0x29D61496 title="Sign in - Google Accounts"
       Chrome_RenderWidgetHostHWND hwnd=0x621812EE title="Chrome Legacy Window"
       Intermediate D3D Window hwnd=0x3D6F1490
```

**Steps** (`visible` is `IsWindowVisible(WRY_WEBVIEW)`; identical outcomes in both runs):

| Step | Posted at (client) | Result | Same WRY HWND | Process |
| --- | --- | --- | --- | --- |
| rail → App Store | `(32, 32)` | `visible=False` immediately (first poll) | yes, `0x3D4F1490` / `0x3EFC1338` | alive |
| rail → Gmail | `(32, 80)` | `visible=True` | yes, same HWND and rect | alive |
| rail → Settings | `(32, 720)` = h−80 | `visible=False` | yes | alive |
| Escape | — | `visible=True` | yes | alive |
| rail → notifications | `(32, 768)` = h−32 | `visible=False` | yes | alive |
| notifications again | `(32, 768)` | `visible=True` (toggle) | yes | alive |
| notifications, second open | `(32, 768)` | `visible=False` | yes | alive |
| Escape | — | `visible=True` | yes | alive |

Run 2's `VERDICT_FLAGS`: `store_hides=true, gmail_shows=true, settings_hides=true,
settings_escape_shows=true, notifications_hides=true, notifications_toggle_shows=true,
notifications_escape_shows=true, single_wry_hwnd=["0x3EFC1338"]`.

**Same page, not a new one.** Across both runs the only `WRY_WEBVIEW` HWND ever enumerated per run
was the baseline one (`0x3D4F1490`, `0x3EFC1338`), with the same rect while hidden and after
return. `Chrome_WidgetWin_1` kept its HWND, title ("Sign in - Google Accounts") and rect through
hide/show; nothing was re-parented into a new child. A detail worth recording: while the page was
hidden, `Chrome_RenderWidgetHostHWND` was **absent** from `EnumChildWindows`, while
`Chrome_WidgetWin_0`, `Chrome_WidgetWin_1` and `Intermediate D3D Window` stayed enumerated (all
`visible=False`); after return the render-widget host was enumerated again with the **same HWND**
(`0x621812EE`) and title. Whether Chromium transiently unparents that window or Windows hides it
from child enumeration is not determinable from handles alone — but the wrapper and browser widget
that identify the page persisted throughout.

**Close and cleanup** (both runs): `WM_CLOSE` → exit code **0 in 0.3 s**; `instance.json` present
before close and **gone after**; `session.json` rewritten (543 B run 1, 541 B run 2). Process
counts: 0 `station-shell` / 0 `msedgewebview2` before; **1 / 9** while running (after all the
hide/show cycles — no new browser tree); **0 / 0** after cleanup. Scratch dirs (and the probe
scripts) deleted and verified absent. WebView2's exe-relative user-data folder
(`rust/target/release/station-shell.exe.WebView2`, inside the gitignored `target/`) pre-existed
these runs and was left in place; no source file was edited and nothing was rebuilt.

## Verdict per claim

- **Rail → App Store hides the live page:** observed. `(32,32)`; `WRY_WEBVIEW` visible true → false
  with the process alive, same HWND, same rect.
- **Rail → installed app shows it again:** observed. `(32,80)`; visible false → true, the same
  `WRY_WEBVIEW` HWND and rect, and the same `Chrome_WidgetWin_1` HWND/title.
- **Settings hides the page and Escape restores it:** observed. `(32, 720)` hid it; `VK_ESCAPE`
  (keydown+keyup) showed it again with the same HWND.
- **Notification panel hides the page and Escape restores it:** observed. `(32, 768)` hid it;
  Escape restored; a second press on the same point also restored it (the panel toggles).

## What this does not prove

- **No pixels.** Win32 cannot read what Rune painted; the store, Settings and panel rendering are
  inferred from the clicks landing in the rail's own 40 pt rows and from the documented handlers
  (and Escape's documented close), not from a screenshot. The notification panel had no
  notifications seeded, so it would have shown its empty state.
- **DOM/history/scroll survival is not observable this way.** Same `WRY_WEBVIEW` and
  `Chrome_WidgetWin_1` HWNDs, unchanged title/rect and no new page child are the evidence; a
  reload inside the same widget cannot be strictly excluded from window handles alone.
- The click coordinates are layout-derived and were exact on the first candidate in both runs;
  the fallback grid was not exercised. No taskbar/title-bar or DWM behaviour was inspected, and
  no keyboard focus was involved beyond the posted `WM_KEYDOWN`/`WM_KEYUP`.
