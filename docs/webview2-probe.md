# WebView2 probe — does a page render in the desktop session? (2026-11-18)

**Verdict: pages render on the desktop session now.** One launch of the existing release binary,
seeded with Gmail, produced a full `msedgewebview2` process tree, a `WRY_WEBVIEW` child window,
and a Chromium render widget whose window title carried real page content
("Sign in - Google Accounts"). No `0x80070057`, no `no web view for` line.

## Machine / session facts (read-only checks, 2026-11-18)

- Binary used as-is: `rust/target/release/station-shell.exe`, **29,301,760 B** (no rebuild; this
  is a newer build than the 29,135,872 B binary recorded earlier today — the tree changed
  between slices).
- Session: `qwinsta` → `console  asd  1  Active` — interactive console session 1 (not a service
  session). `SESSIONNAME=Console`.
- WebView2 runtime present: `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` →
  `pv = 157.0.4341.46` (the HKCU twin is absent). Sole installed runtime:
  `C:\Program Files (x86)\Microsoft\EdgeWebView\Application\157.0.4341.46`.
- The shell does **not** honour a browser-folder option: `webview.rs:844-864` builds with
  `with_url` / init script / ipc / new-window-req / `with_bounds` / `with_visible(false)` /
  `with_focused(false)` / `build_as_child` — no `with_browser_executable_folder`. The planned
  `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` experiment was therefore moot and not run (the runtime
  was present and worked).

## What I ran

Scratch session (deleted after; never the user profile) at
`%TEMP%\wv2-probe\session.json` — Gmail is manifest id 14
(`packages/app/manifests/definitions/14.json`, `google-account` preset):

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

Launch: `STATION_SESSION_PATH=<scratch>\session.json rust/target/release/station-shell.exe`,
stderr captured, 15 s bounded wait, then read-only observation.

## Exact outputs

**stderr (run 1, abridged):** no `no web view for` line and no attach error. Present instead:
`window icon 32×32/64×64 could not be created` (existing headless-session limitation), ~130
`no icon for https://cdn.filestackcontent.com/…: status code 404` (store icon CDN unreachable
from this network — letter avatars, the documented degraded path), and
`rune: GPU "Radeon RX 580 Series" (wgpu; Dx12, DiscreteGpu, …): the GPU driving the primary display`.

**Process tree** (`Get-CimInstance Win32_Process`): 8 `msedgewebview2.exe`, all
`…\EdgeWebView\Application\157.0.4341.46\msedgewebview2.exe`:

| PID | Parent | Role |
| --- | --- | --- |
| 138624 | 97152 (shell) | browser (`--embedded-browser-webview=1 --webview-exe-name=station-shell.exe`) |
| 134084 | 138624 | crashpad-handler |
| 84184 | 138624 | gpu-process |
| 139944 / 144076 / 119844 | 138624 | utility: network / storage / audio |
| 126080 / 117824 | 138624 | renderer ×2 |

**Child windows** (PowerShell + Win32 `EnumWindows`/`EnumChildWindows` P/Invoke on pid 97152):

```
TOP hwnd=0x2C7A0244 pid=97152 class='Window Class' text='Station' rect=(130,130)-(1346,969)
CHILD hwnd=0x18F50B68 pid=97152 class='WRY_WEBVIEW' text='' rect=(202,205)-(1338,961)
GCHILD hwnd=0x29F11492 pid=97152 class='Chrome_WidgetWin_0' rect=(202,205)-(1338,961)
GCHILD hwnd=0x1B580F92 pid=138624 class='Chrome_WidgetWin_1' text='Sign in - Google Accounts' rect=(202,205)-(1338,961)
GCHILD hwnd=0x3A0D0B08 pid=138624 class='Chrome_RenderWidgetHostHWND' text='Chrome Legacy Window' rect=(202,205)-(1338,961)
GCHILD hwnd=0x16E8030C pid=84184 class='Intermediate D3D Window' rect=(202,205)-(1338,961)
```

`IsWindowVisible` on the top window, `WRY_WEBVIEW` and `Chrome_WidgetWin_1`: all `True`, none
iconic. The pane rect sits inside the content area (rail + top bar left to Rune), matching
`docs/rust-rune-port.md` D4's geometry.

Page identity: `mail.google.com` redirected to Google's signed-out account picker
(`Sign in - Google Accounts` in the widget title) — the expected landing for Gmail without
identity config (`docs/rust-rune-port.md` D8). Real web content reached the render widget; the
page rendered and was visible.

## Conclusion and what I could not determine

- **Pages render on the desktop session.** The blocked state reported by every slice since 7
  (`0x80070057` from the attach path, no `msedgewebview2` process, 10/10) did not reproduce.
- Could not determine **why the earlier runs failed**: this is n=1 today, and the binary changed
  between the failing slices and this probe (29,135,872 → 29,301,760 B), so a build-side fix
  cannot be ruled out. Machine facts are unchanged from the failing era: same runtime version
  (157.0.4341.46), same interactive console session, same session 1.
- Could not determine page rendering **inside** the widget pixels (no screenshot taken —
  read-only observation); the visible Chromium render widget + D3D window + page-process tree +
  window title are the evidence, not a pixel check.
- This build logs no `navigation started` / `page load finished` lines (those came from an
  earlier instrumented build), so absence of navigation logs is not evidence either way.

## Cleanup

Probe tree killed via PowerShell `Stop-Process` (browser + shell; subprocesses exited with the
browser) — final check printed `NONE REMAIN` for `station-shell|msedgewebview2`. Scratch dir
`%TEMP%\wv2-probe\` (session.json, instance.json the shell writes beside the session path,
probe script, run log) deleted, verified gone. Note: WebView2's default user-data folder
`rust/target/release/station-shell.exe.WebView2` (exe-relative, inside the gitignored
`target/`) was created by the runtime and left in place — not my scratch, shared by smoke runs.
