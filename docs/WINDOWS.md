# Porting Glyphio to Windows

Status: **planning**. Nothing Windows-specific is implemented yet. This document is the survey
that a port starts from — what already works, what has to be written, and where the surprises
are.

## The short version

Most of Glyphio is already portable. Text expansion is nearly free. Capture is the work, and
one specific piece of it — the interactive picker — is the long pole, because macOS hands us
something Windows simply doesn't have.

## What ports unchanged

- **The Tauri shell** — windows, tray, global shortcuts, deep links, the updater.
- **All of `ui/`** — settings, editor, history, palette, banner compositing.
- **`snippet-store`, `sync-client`, `sync-proto`** — pure Rust, no platform calls.
- **History** — SQLite via `rusqlite` with the bundled build.
- **Clipboard history** — the store, retention, dedupe, search and picker are all portable.
  Its platform surface is four functions in `clipboard/platform.rs` (change counter, concealed
  check, foreground app, paste keystroke) and **the Windows half is already written** — via
  `GetClipboardSequenceNumber`, `ExcludeClipboardContentFromMonitorProcessing` and `SendInput`.
  It has never been compiled, so treat it as a first draft, not a finished port; it also needs
  `windows-sys` adding as a `[target.'cfg(windows)'.dependencies]` entry.
- **Capture orchestration** (`src/capture.rs` itself) — mode selection, delivery, encoding.

## Text expansion: nearly free

The vendored espanso fork still carries upstream's Windows support. `win32` implementations
exist for every platform-facing crate:

```
espanso-inject/src/win32     espanso-ui/src/win32       espanso-detect/src/win32
espanso-clipboard/src/win32  espanso-info/src/win32
```

with 31 files under `cfg(target_os = "windows")`. We are not porting text expansion; we are
building it for a second target.

Two things do need doing:

- The win32 bridges are C++ compiled through `cc`, so the build needs MSVC build tools.
- `scripts/build-engine.sh` is bash and macOS-shaped. It needs a PowerShell sibling or,
  better, to be rewritten as a small Rust `xtask` that both platforms share.

## Capture: the actual work

Roughly 1,200 lines are macOS to the bone.

### `capture/backend.rs` (~544 lines) → Windows.Graphics.Capture

Screen and window capture, multi-monitor compositing, DPI handling. The compositing and
rect-intersection logic is portable in shape but not in API. Windows.Graphics.Capture is the
modern, permission-free equivalent of ScreenCaptureKit.

**Mixed-DPI is the trap.** The existing code resolves a rect spanning two displays to the
sharpest scale involved and upscales the coarser piece. Windows has per-monitor DPI awareness
with its own rules; expect this to need rework rather than translation.

### `capture/ax.rs` (~498 lines) → UI Automation

Reads page identity (URL, title, profile) for the banner. On macOS, Chrome answers from the
window's `AXDocument` in ~100µs with no accessibility tree built — measured, and the reason
this feature is cheap enough to ship.

**Expect this to be worse on Windows.** Chrome's UIA tree is less obliging than its AX tree,
and the address bar is typically read via a value pattern on a control found by traversal
rather than a single attribute on the window. Budget for it being slower, and keep the
existing rule that browser details are opt-in.

### `capture/scroll.rs` (~275 lines) → `SendInput`

Only `post_scroll`, `warp_cursor` and `cursor_position` are Quartz. `stitch`, `band_gray`,
`mad` and `frames_identical` are pure image work with existing tests, and port as-is.

### `scripts/ocr.swift` → Windows.Media.Ocr

On-image OCR. Comparable scope to the Swift version. WinRT's OCR API is a reasonable match.

## The long pole: there is no interactive picker

`snip` and `fullWindow` currently shell out to `/usr/sbin/screencapture -i`, which gives us
Apple's own picker for free: dimmed overlay across every display, hover-highlighting of
windows, drag-to-region, Esc to cancel, correct on mixed-DPI multi-monitor setups. One
`Command::new` call.

**Windows has no equivalent** we can drive. Snipping Tool cannot be invoked to hand a region
back to a calling process in a usable way. This has to be built: a transparent always-on-top
overlay window per display, hit-testing against the window list for highlight, drag selection,
and correct behaviour when displays have different scale factors.

Realistically this is comparable in effort to the rest of the capture backend combined, and
it's the piece most likely to feel subtly wrong if rushed. It is also unavoidable: region
capture is the most-used mode.

## What gets simpler

- **No TCC.** The entire macOS permissions surface — Screen Recording, Accessibility, the
  relaunch dance, the responsible-process problem — has no Windows counterpart. Delete it.
- **No `AXEnhancedUserInterface` opt-in** and no cooldown thread restoring it.
- **No Spaces**, so the full-screen editor placement problem disappears.

## Loose ends

- **UI copy**: ~30 hardcoded `⌘` references and ~26 macOS-permission strings need platform
  branching. Worth a small helper rather than 56 conditionals.
- **Default hotkeys**: `Alt+Shift+…` is fine on Windows, but check for conflicts with common
  IME and accessibility bindings.
- **Code signing**: Windows wants its own certificate. Unsigned installers trigger SmartScreen
  much as unsigned apps trigger Gatekeeper — same problem, different vendor, also money.
- **Paths**: `paths.rs` uses `dirs`, which already resolves correctly per platform. Verify the
  engine's `ESPANSO_*` directories land somewhere sensible under `%APPDATA%`.
- **The updater** already targets `darwin-aarch64` explicitly in `latest.json`; adding
  `windows-x86_64` is a manifest change, not a code change.

## Suggested order

1. Make the tree *compile* on Windows: gate the macOS modules, stub the platform seams.
2. Engine sidecar building and running under the supervisor.
3. Non-interactive capture (`visible`, `frontWindow`) — proves the pixel path end to end.
4. The picker overlay.
5. UI Automation page identity.
6. Scroll injection.
7. OCR.

Steps 1–3 get to something demonstrable quickly. Step 4 is where the time goes.
