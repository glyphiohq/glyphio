# Glyphio — Phase 1 build notes

Glyphio is an enterprise-internal macOS app merging **text expansion** (a thin-branding fork of
espanso, GPL-3.0) with **screenshot capture / annotate / redact** (ported from the author-owned
Checkpoint Chrome extension), wrapped in a **Tauri** shell. Local-first; the local data
architecture for future team sync is built now, only the remote connection (Okta + AWS) is
deferred.

This document covers, per the phase brief: what was rebranded, which espanso↔Tauri integration
approach was used and why, which full-page-capture approach was used and why, and what is
explicitly deferred.

---

## Repository layout

```
glyphio/
├─ espanso/                     # vendored espanso fork (upstream dev @ f239adb; see UPSTREAM_VERSION.txt)
│                               #   builds the headless `glyphio-espanso` sidecar binary
├─ src-tauri/                   # Tauri v2 Rust backend
│  ├─ crates/snippet-store/     # standalone crate: SQLite source-of-truth + espanso YAML generator
│  ├─ binaries/                 # bundled sidecar (glyphio-espanso-<triple>)
│  ├─ src/                      # app: paths, settings, espanso supervisor, snippets, history,
│  │                            #      capture (ScreenCaptureKit), shortcuts, tray, windows, commands
│  └─ build.rs, tauri.conf.json, capabilities/
├─ ui/                          # webview frontend (ported Checkpoint vanilla JS, chrome.* → Tauri)
│  ├─ config.js, shared/        # ported config + shortcuts helpers
│  ├─ settings/                 # NEW: snippet manager + settings (main window)
│  ├─ editor/                   # ported preview.js editor (banner + crop/redact/draw/text)
│  ├─ history/                  # ported history grid
│  └─ snip-overlay/             # ported snip overlay (region select — see deferred)
└─ docs/PHASE1.md               # this file
```

## Build & run (macOS Apple Silicon)

Prereqs: Rust stable, Node 18+, Command Line Tools (full Xcode NOT required — see "Swift linking").

```bash
# 1. Build + sign the headless engine sidecar (no modulo/wxWidgets). This builds the espanso fork,
#    copies it in with Tauri's sidecar naming (glyphio-engine-<triple>), and code-signs it.
npm install
npm run engine                              # debug   (scripts/build-engine.sh)
#   or: npm run engine:release              # release (scripts/build-engine.sh --release)

# 2. Run the app  (npm run dev re-signs the sidecar first via the `predev` hook)
npm run dev                                 # tauri dev
#   or: npm run build                       # bundle Glyphio.app + dmg
```

First launch prompts for two macOS permissions (see "Permissions"). The Settings/snippet
window opens; the tray shows Capture / History / Settings / Quit.

### Dev code-signing (why the Accessibility grant used to keep resetting)

macOS TCC keys the Accessibility grant to the engine binary's **code identity**. An unsigned
binary's identity (cdhash) changes on every `cargo build`, so a freshly-built engine is treated as
a new, untrusted program — System Settings still shows the old entry toggled on, but keystroke
detection silently no-ops and expansions stop working.

`scripts/dev-sign.sh` fixes this by signing the engine with a **stable self-signed identity**
("Glyphio Dev", stored in a dedicated `glyphio-dev.keychain-db` so the flow is non-interactive).
The signature's Designated Requirement is certificate-based (`identifier
"io.glyphio.app.engine" and certificate leaf = H"…"`), which is constant across rebuilds.
**Grant Accessibility to `glyphio-engine` once**; it then survives rebuilds as long as the sidecar
keeps being signed (the `engine`/`predev`/`sign` npm scripts do this automatically). Run
`npm run sign -- --reset` to recreate the identity from scratch. In dev the Accessibility list
shows `glyphio-engine`; a signed prod `.app` can present it as "Glyphio Engine".

The engine also **re-checks Accessibility every ~2s** (fork deviation in
`espanso/src/cli/daemon/mod.rs`): granting it while the app is running now auto-restarts the worker
so the keystroke event-tap is recreated under trust — no manual "Restart engine" needed, and the
Settings banner clears itself.

---

## A. espanso fork + rebrand

**Fork.** Upstream espanso `dev` (SHA `f239adb`) is vendored under `espanso/` (nested `.git`
removed; provenance in `espanso/UPSTREAM_VERSION.txt`). GPL-3.0 `LICENSE` and all upstream
copyright notices are preserved.

**Core vs shell.** Core engine crates — `espanso-detect`, `-inject`, `-match`, `-engine`,
`-config`, `-render`, `-ipc` — are **untouched** (minimises the diff vs upstream so future
security/bugfixes rebase cleanly). The only rebrand touch-points are shell-facing.

**What was rebranded.** Because espanso runs **headless** as a sidecar, Glyphio's Tauri shell
owns everything a user sees — tray, About, notifications, product name. So the espanso rebrand
is deliberately minimal:
- The sidecar binary is named **`glyphio-espanso`**.
- espanso's own tray icon and notifications are **disabled** via the generated
  `config/default.yml` (`show_icon: false`, `show_notifications: false`), so only Glyphio's
  tray appears. (Config is generated by `snippet-store`, see below.)
- User-facing product strings in the frontend (`ui/config.js` `name`/`shortName`) are
  `"Glyphio"`; window titles are "Glyphio …".

No internal espanso module/function was renamed for cosmetics. **Deviations from upstream:**
only the vendoring (removed `.git`) and the headless build flags — no source edits to the
engine. Built with `--no-default-features --features native-tls`, which drops `modulo`
(wxWidgets); Glyphio never uses espanso's Forms/Search GUI, and this removes the only heavy
C++ dependency (no full Xcode needed to build espanso).

## Integration approach: **managed sidecar** (decided with the human)

The Tauri app **supervises** the `glyphio-espanso` binary as a child process
(`src-tauri/src/espanso.rs`), running espanso's hidden foreground `daemon` subcommand, pointed
at an **isolated config dir** via `ESPANSO_CONFIG_DIR` / `ESPANSO_RUNTIME_DIR` /
`ESPANSO_PACKAGE_DIR` (`~/Library/Application Support/Glyphio/espanso/…`). The daemon
file-watches that dir and hot-reloads.

**Why sidecar over embedding espanso as a library:** espanso is architected as a
daemon/worker process pair (with a macOS `fork` model, its own event loop, IPC sockets, and
tray). Embedding its crates in-process would fight that design and produce a large, hard-to-
rebase diff. The sidecar keeps the engine byte-for-byte upstream, matches how espanso is meant
to run, and needs **zero** espanso code changes — the config-file interface is the whole
integration surface. Trade-off: two processes to lifecycle-manage (handled by the supervisor:
spawn on launch, kill on exit).

*Verified:* the app boots, spawns the daemon, which reads the generated config and hot-reloads
(`espanso match list` against the generated dir lists the snippets; the daemon logs confirm
`CocoaSource`/`MacInjector` native injection is active).

## B. Local snippet store + YAML generator

`src-tauri/crates/snippet-store` is a **standalone crate** (no Tauri/espanso dependency) so
Phase 2's sync client can consume it unchanged. SQLite is the **source of truth**; espanso's
YAML is a generated, disposable artifact.

- **Schema:** `id, trigger, replacement, variables (JSON), app_scope, owner, team, updated_at,
  version, deleted_at`. `owner`/`team` default to a local `personal` scope and
  `updated_at`/`version`/`deleted_at` (soft-delete tombstones) exist now so enabling real team
  sync later is **additive, not a migration**.
- **Interface:** `open`, `list`, `get`, `create`, `update`, `soft_delete`, an `add_change_listener`
  hook (Phase 2 sync + UI refresh both subscribe), and `render_yaml`.
- **Generator:** every mutation (via the Tauri commands in `snippets` + `commands.rs`)
  regenerates `match/glyphio.yml` and the headless `config/default.yml`, written **atomically**
  (temp + rename) so the watcher never reads a half file. All snippet editing goes through the
  store — never hand-edited YAML.
- Unit-tested (CRUD + soft-delete + change events + YAML round-trip).

## C. Checkpoint capture / edit / history (native port)

The editor + banner (`ui/editor/editor.js`, ported from Checkpoint `preview.js`) and the
history grid (`ui/history/history.js`) are the portable Canvas2D/DOM code, copied and rewired:
`chrome.storage`→`get_settings`/`localStorage`, `chrome.downloads`→Tauri save dialog +
`save_file`, `chrome.tabs`→`open_window`/`open_capture`, IndexedDB→native history store,
`chrome.runtime` messaging removed. **Edit tools kept as-is:** Crop, Redact (black-out + blur),
Draw (rect/arrow/freehand, six-colour palette, undo), Text (click-to-place, size, undo), each
toggleable in Settings. Banner (timestamp + window/app title + free-text note) is composed on
canvas exactly as Checkpoint did, default-on/off togglable.

**History** (`src-tauri/src/history.rs`) replaces IndexedDB with SQLite metadata + on-disk PNGs
under `…/Glyphio/history/`, preserving Checkpoint's behaviour: grid, per-entry
Open/Copy/Download/Delete, Clear-all, and **oldest-first eviction at 50 captures OR 200 MB**.
History is device-local and never synced.

## Full-page capture approach: **native full-window via ScreenCaptureKit** (decided with the human)

"Full page" doesn't map onto a native app (no DOM to scroll-and-stitch as the browser
extension did). We use `screencapturekit` (SCScreenshotManager, macOS 14+) in
`src-tauri/src/capture/backend.rs`:
- **visible** → main display capture.
- **fullWindow** → the frontmost non-Glyphio window as rendered ("full page" = "full window").
- **snip** → display capture; the user crops with the editor's Crop tool (drag-select overlay
  is deferred — see below).

**Why native full-window, not CDP:** driving a browser via the Chrome DevTools Protocol for
true beyond-viewport capture would require launching/attaching a browser with remote debugging
and is browser-specific and fragile. Native full-window works for **every** app with no browser
dependency; its only limitation is it can't capture scrolled-away content. That trade was
accepted for Phase 1.

## D. Tauri shell integration

Menu-bar/tray (`tray.rs`) with Capture (visible/snip/full) / History / Settings / Quit; global
hotkeys mirroring Checkpoint (Alt+Shift+S/V/X + H) via `tauri-plugin-global-shortcut`
(`shortcuts.rs`, re-registered live on settings change); a Settings window (`ui/settings/`)
covering the snippet manager + capture/edit/banner/history settings; capture→edit→history and
snippet-CRUD→YAML→live-reload flows wired end-to-end. Menu-bar app (Accessory activation policy,
no dock icon).

## Permissions (macOS)

- **Accessibility** — espanso keystroke injection/detection. Prompted by the daemon on first run.
- **Screen Recording (TCC)** — ScreenCaptureKit. Prompted on the first capture.

Phase 1 relies on the OS prompts; a guided first-run onboarding UI is a polish follow-up.

## Swift linking note (build environment)

The `screencapturekit` crate compiles a **Swift** bridge and links against the Swift runtime.
On Command Line Tools-only machines (no full Xcode) `build.rs` adds the toolchain's Swift lib
dirs to the link search path and sets the runtime rpath to the OS Swift runtime (`/usr/lib/swift`)
— so it links and runs with a single Swift runtime copy (no duplicate-class warnings). Harmless
when full Xcode is present.

---

## Deferred / TODO

**Product/legal**
- **OSS/legal sign-off** for the internal GPL fork — flag to the reference enterprise deployment OSS/compliance before wide
  rollout (does not block building/internal use per the FSF internal-use guidance).
- No telemetry / auto-update / non-the reference enterprise deployment network calls (guardrail upheld). No Okta/AWS yet.

**Functional follow-ups — ALL RESOLVED in Phase 2 (2026-07-02):**
- ~~Snip region overlay~~ — superseded: snip/window capture use the native `screencapture -i`
  interactive picker (Apple's own multi-monitor drag-select overlay), which is strictly better
  than the ported DOM overlay; the dead `ui/snip-overlay/` port was removed.
- ~~`app_scope` enforcement~~ — enforced. The generator now emits per-scope
  `match/_scoped_<n>.yml` files (underscore-prefixed, skipped by espanso's default include
  glob) activated by generated `config/app_<n>.yml` app configs (`filter_exec`/`filter_title`
  + `extra_includes`). Scope syntax: bare app name, `exec:<regex>`, `title:<regex>` — editable
  in the snippet editor's "App scope" section.
- ~~Capture DPR~~ — real backing scale reported (and a bonus fix: SCK "visible" captures were
  requesting point-dimensions, silently downscaling Retina captures 2×; now pixel-exact).
- ~~Icon~~ — real generated mark (`scripts/gen-icon.swift` → `npx tauri icon`).
- ~~Guided first-run permissions~~ — Screen Recording status/request/System-Settings commands
  + a banner mirroring the Accessibility one.

**Verified this phase:** espanso headless build; snippet-store crate (unit tests); the
store→YAML→espanso pipeline (`espanso match list`); the Tauri app builds and boots; the
supervisor spawns the daemon and it hot-reloads the generated config; the SCK backend links and
the app boots cleanly. **Not yet verified interactively (needs a GUI session + granted
permissions):** an end-to-end typing-expansion test, live screen captures, and the editor tools
in the running webview.

## Later phases (context)

- **Phase 2:** Okta OIDC/PKCE + AWS sync backend; sync client plugs into `snippet-store`
  (LWW on `updated_at`/`version`); team scope becomes real; personal snippets stay local.
- **Phase 3:** Windows port (espanso core + Tauri + `Windows.Graphics.Capture`/`xcap`).
- **Phase 4:** code signing/notarization, production Okta app, formal OSS sign-off, rollout.
