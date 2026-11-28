# Window icon probe — does the window carry Station's icon? (2026-11-18)

**Verdict: not observed — the window carries no icon.** The running release binary's own
`CreateIconFromResourceEx` calls returned null for both sizes, so `WM_SETICON` was never sent:
`WM_GETICON` is null for `ICON_SMALL`, `ICON_BIG` and `ICON_SMALL2`, and both
`GetClassLongPtr(GCLP_HICON/GCLP_HICONSM)` class-icon fallbacks are null too. The executable
itself has no icon resource either (`ExtractIconExW` returned 0), so there is nothing to fall
fall back to. This is **not** the session limitation the earlier webview probe assumed: third-party
windows in the same console session return icons from both calls, and an independent process
here creates and inspects HICONs of exactly the sizes the shell asks for.

> **Update (same day, 19:29):** the defect below was fixed in `platform_icon.rs` and re-probed —
> the window now carries both icons. The after-fix evidence and verdict are in the
> **"After the fix"** section at the end; the evidence above is kept as the before state.

## Machine / session facts (read-only checks, 2026-11-18)

- Binary used as-is, **not rebuilt**: `rust/target/release/station-shell.exe`, **29,319,680 B**,
  mtime `2026-11-18 19:08:42`, SHA-256
  `ff714fe3209f9e5d70616275242d9c2a76d36b8c60e7c49bd88023db2ba25b38` (x64 PE, machine `0x8664`).
- Fresheness caveat: one source file is newer than the exe —
  `rust/crates/station-shell/src/tests_notify_click.rs` (19:09, a test file; another session is
  editing comments/docs). No shell source used by the icon path is newer (`platform_icon.rs`
  11:31, `webview.rs` 19:02). The probe used the binary as instructed and did not build.
- Session: `qwinsta` → `console  asd  1  Active`; `SESSIONNAME=Console` — interactive console
  session 1, not a service session.
- OS: `Windows-10-10.0.19045-SP0`, AMD64.
- WebView2 runtime: sole installed version **157.0.4341.46**
  (`HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-…}` `pv`), same as the
  webview probe.
- The icon code under test is `rust/crates/station-shell/src/platform_icon.rs`: it decodes
  `packages/app/build/icon_512x512.png` at compile time, builds a 40-byte-BITMAPINFOHEADER
  BGRA buffer with a doubled `biHeight` and an all-zero AND mask, and calls
  `CreateIconFromResourceEx` for 32×32 (`ICON_SMALL`) and 64×64 (`ICON_BIG`), sending one
  `WM_SETICON` per size. A null HICON logs `window icon {w}×{h} could not be created` and
  skips that `WM_SETICON`.

## What I ran

Two complete launches (runs at 19:10:07 and 19:13:09), both with scratch session paths — never
the user profile. Scratch dirs `%TEMP%\win-icon-probe-20261118-191007\` and
`%TEMP%\win-icon-probe3-20261118-191309\`, each holding the Gmail session shape from
`docs/webview2-probe.md`:

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

Launch (same environment, stderr/stdout captured to files in the scratch dir):

```
STATION_SESSION_PATH=%TEMP%\win-icon-probe-<ts>\session.json rust\target\release\station-shell.exe
```

Observation was read-only Python 3.13 + ctypes in that session: `EnumWindows` /
`EnumChildWindows` + `GetWindowThreadProcessId` for the shell pid, `SendMessageW(WM_GETICON)`
with `ICON_SMALL`/`ICON_BIG`/`ICON_SMALL2`, `GetClassLongPtrW(GCLP_HICONSM/GCLP_HICON)`,
`GetIconInfo` on any non-null handle, and `Get-CimInstance Win32_Process` for process counts.
No screenshot, no taskbar inspection, no build, no code edits.

## Exact outputs

**Top window** (both runs, found on the first poll — 0.4 s in run 1 — class `Window Class`,
title `Station`):

```
run 1  hwnd=0x44931494  visible=True  rect=(26,26)-(1242,865)
run 2  hwnd=0x31431434  visible=True  rect=(52,52)-(1268,891)
```

Other top-level windows of the shell process: `RuneSession` (16×16, hidden), `Winit Thread Event
Target` (16×16, visible), `IME` ("Default IME", empty rect, hidden).

**`WRY_WEBVIEW` child** (visible, geometry matches the webview probe — same 1136×756 pane):

```
run 1  WRY_WEBVIEW   visible=True  rect=(98,101)-(1234,857)
run 2  WRY_WEBVIEW   visible=True  rect=(124,127)-(1260,883)   (12 s after launch)
```

Run 2's full child tree at 12 s (same shape the webview probe reported):

```
WRY_WEBVIEW              visible=True  rect=(124,127)-(1260,883)
Chrome_WidgetWin_0       visible=True  rect=(124,127)-(1260,883)
Chrome_WidgetWin_1       visible=True  rect=(124,127)-(1260,883)  title="Sign in - Google Accounts"
Chrome_RenderWidgetHostHWND visible=True rect=(124,127)-(1260,883) title="Chrome Legacy Window"
Intermediate D3D Window  visible=True  rect=(124,127)-(1260,883)
```

**Icon calls on the top window** — identical in both complete runs:

| Call | Return | GetIconInfo |
| --- | --- | --- |
| `WM_GETICON` `ICON_SMALL` (0) | `0` (null) | not applicable |
| `WM_GETICON` `ICON_BIG` (1) | `0` (null) | not applicable |
| `WM_GETICON` `ICON_SMALL2` (2) (extra) | `0` (null) | not applicable |
| `GetClassLongPtr` `GCLP_HICONSM` (-34) | `0` (null) | not applicable |
| `GetClassLongPtr` `GCLP_HICON` (-14) | `0` (null) | not applicable |

There was no non-null `HICON` on this window, so no `GetIconInfo` bitmap dimensions exist to
report for it. The window also reports `iconic=False` and `visible=True` at the time of every
call, so the nulls are not "not yet shown" artifacts.

**Shell stderr** (run 2, the lines that decide this), immediately after the window appeared:

```
station-shell: window icon 32×32 could not be created
station-shell: window icon 64×64 could not be created
```

Those two lines are exactly the `CreateIconFromResourceEx`-returned-null branch
(`platform_icon.rs:119`), one per requested size — so neither `WM_SETICON` was reached. The rest
of stderr was the expected noise: ~60 `no icon for https://cdn.filestackcontent.com/…: status
code 404` lines and `rune: GPU "Radeon RX 580 Series" …`. No `no web view for` line and no
`window has no native handle` line.

**Executable icon resource:** `ExtractIconExW(station-shell.exe, 0, …)` returned `0` — the exe
carries no icon at all, so there is no window-icon fallback from the image either.

## Controls that make the nulls meaningful

All in the same console session, same harness, same calls:

- Other processes' windows answer both paths: `obj_Form` ("Shollu v3.10") → `WM_GETICON
  ICON_BIG` = 32×32 HICON (`GetIconInfo`: color 32×32 32 bpp, mask 32×32 1 bpp), `ICON_SMALL2`
  = 16×16; a Chrome window → class icons `GCLP_HICON` 32×32 / `GCLP_HICONSM` 16×16 and
  `WM_GETICON ICON_BIG/ICON_SMALL` 48×48. So `WM_GETICON`, `GetClassLongPtr` and `GetIconInfo`
  all work cross-process here; the Station window's zeros are its state, not the probe's.
- The shell's buffer shape is accepted by this machine: an independent process here built the
  same 40-byte-header, doubled-`biHeight`, premultiplied-BGRA, zero-AND-mask buffer from the
  **real** `packages/app/build/icon_512x512.png` pixels (Pillow Lanczos) and called
  `CreateIconFromResourceEx` with the same `0x00030000` / size parameters:
  32×32 → 4,264-byte buffer, non-null HICON; 64×64 → 16,936-byte buffer, non-null HICON
  (`GetLastError` 0). This rules out "the session cannot create icons" and "the buffer layout is
  rejected"; the failure is specific to the shell process or its call.

## Clean close, instance file, process counts

- `WM_CLOSE` posted to the top window: process exited **code 0 in 0.3 s** in both runs.
- `instance.json` was present beside the session file before close (`instance.json`,
  `session.json`, logs) and **gone after close** in both runs. `session.json` was rewritten
  (541 B) with the restored placement, as designed.
- Process counts (`Get-CimInstance`): before 0 `station-shell` / 0 `msedgewebview2`; during run 1
  → 1 / 6, during run 2 at 12 s → 1 / 8 (browser, crashpad, gpu, network/storage/audio
  utilities, 2 renderers); after cleanup → **0 / 0**. `NONE REMAIN` verified.
- Scratch dirs deleted and verified absent. One earlier harness attempt aborted in its own
  child-enumeration step; its shell window was closed and its processes and scratch dir were
  cleaned the same way, and no observations from it are used above.

## Conclusion and what I could not determine

- The window does **not** carry a small or big icon from `WM_SETICON` on this build/machine:
  the icon creation failed before the messages were sent, and the class/exe fallbacks are also
  empty. `rust/README.md`'s "the window carries Station's icon (title bar + taskbar)" is not
  true of a running window here; the only evidence for the icon remains the source and unit
  tests, which do not exercise `CreateIconFromResourceEx` or `WM_SETICON`. The earlier webview
  probe already saw the same two failure lines with the 29,301,760 B binary, so this is not new
  to the 29,319,680 B build (that probe called it a "headless-session limitation"; this session
  is interactive and the controls above contradict that explanation).
- Could not determine **why** the shell's `CreateIconFromResourceEx` calls return null: the same
  call with the same buffer shape and the real asset's pixels succeeds in an independent process
  in the same session, so it is not the asset, the layout, the session, or the API binding
  available to this machine. Narrowing it further needs runtime instrumentation of the shell
  process or a rebuild with logging/GetLastError, neither of which this observation does (the
  binary was used as-is).
- This does **not** prove anything about pixel appearance (there is no icon to inspect), taskbar
  rendering specifics (no taskbar/DWM query or screenshot was taken), or whether a rebuild or a
  different Windows version would set the icon. The exe having no icon resource means a
  taskbar fallback would have to be the generic default, but that was not observed.

## After the fix — same harness, rebuilt binary (2026-11-18, run at 19:29)

**Root cause (reproduced).** The old `platform_icon.rs` wrote its 40-byte `BITMAPINFOHEADER` with
`HEADER.to_le_bytes()` where `HEADER: usize = 40`. On x64 that emits **8 bytes** for `biSize`, so
the header was 44 bytes and every following field (width, doubled height, planes, bit count,
compression, size image, …) was shifted four bytes: the call received a 4,268-byte blob instead
of the ICO entry's 4,264. Replaying exactly that byte layout through an independent
`CreateIconFromResourceEx` on this machine returns NULL, while the same pixels packed behind a
proper 40-byte header — and the `.ico`'s own 32/64 entries — return non-null HICONs. The earlier
probe's two failure lines were this malformed header, not the session.

**Fix.** The PNG decode/resize/BGRA path is gone. `platform_icon.rs` embeds
`packages/app/build/icon.ico` (`include_bytes!`), parses the directory (reserved=0, type=1,
16-byte entries, payload inside the file, PNG-compressed entries skipped), and passes the 32×32
and 64×64 entries' own `RT_ICON` bytes to `CreateIconFromResourceEx` (`0x00030000`,
`LR_DEFAULTCOLOR`) before the unchanged `WM_SETICON ICON_SMALL`/`ICON_BIG`. Selection rule: exact
size, else the smallest entry at least as large, else the largest; both exact entries exist here
(4,264 B and 16,936 B payloads, `biHeight` already doubled for the AND mask). New unit tests parse
the embedded ICO and pin the chosen sizes/payloads plus the selection rule and PNG skipping.

**Exact after results** (release `station-shell.exe` 29,073,408 B, mtime 19:27:02; scratch
session, same Gmail seed, 12 s settle, `WM_CLOSE`):

```
WM_GETICON ICON_SMALL  -> non-null HICON; GetIconInfo: color 32×32 32 bpp, mask 32×32 1 bpp
WM_GETICON ICON_BIG    -> non-null HICON; GetIconInfo: color 64×64 32 bpp, mask 64×64 1 bpp
WM_GETICON ICON_SMALL2 -> non-null HICON (same handle as ICON_SMALL); color 32×32
GetClassLongPtr GCLP_HICONSM -> 0 (null)
GetClassLongPtr GCLP_HICON   -> 0 (null)
```

The class icon stays null, so the non-null `WM_GETICON` answers are the per-window `WM_SETICON`
icons (the class fallback is not involved). stderr had **no** `window icon … could not be created`
line (91 lines: store-icon 404s and the GPU line). The window was visible, `iconic=False`, rect
(26,26)-(1242,865). Clean close: exit code 0 in 0.3 s, `instance.json` present before and gone
after, `session.json` rewritten (541 B). Process counts: 0/0 before → 1 shell + 8
`msedgewebview2` at 12 s → **0/0** after cleanup; scratch dir deleted.

**Verdict after the fix: the window carries a small and a big icon from `WM_SETICON`.** Still not
proven: the icons' pixel appearance (no screenshot/readback of the HICON bits), how the taskbar/DWM
renders them at each DPI, and anything about Alt+Tab/task view. The observed sizes are 32×32 for
`ICON_SMALL` and 64×64 for `ICON_BIG`.
