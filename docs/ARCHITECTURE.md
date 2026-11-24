# Glyphio architecture

## Purpose and boundaries

Glyphio is a local-first desktop application that combines text expansion, screenshot capture,
and clipboard history. macOS is the supported production platform. Team sync is optional and
only shares snippet groups deliberately assigned to a team; personal snippets, captures, and
clipboard history remain local.

The application makes no network calls for sync until a user configures it. The update check is
separately user-controllable.

## System overview

```text
                     ┌───────────────────────┐
                     │ Tauri desktop shell   │── optional HTTPS / sync client ──► Protocol-compatible server
                     │ commands, shortcuts,  │                                    (reference: server/)
                     │ tray, permissions     │
                     └───────┬───────┬───────┘
                             │       │
              Tauri IPC      │       │  generated YAML/config
                             ▼       ▼
              ┌────────────────────┐  ┌──────────────────────────┐
              │ UI webview         │  │ glyphio-engine sidecar   │
              │ snippets, editor,  │  │ thin, headless espanso   │
              │ history, settings  │  │ fork; supervised process │
              └─────────┬──────────┘  └──────────────────────────┘
                        │
                        ▼
              ┌────────────────────┐
              │ Local data layer   │
              │ SQLite + local     │
              │ files; source of   │
              │ truth for state    │
              └────────────────────┘
```

## Components and ownership

| Component | Location | Responsibility | License |
| --- | --- | --- | --- |
| Desktop shell | `src-tauri/` | Native integrations, commands, capture, history, sidecar lifecycle | GPL-3.0-or-later |
| Local snippet store | `src-tauri/crates/snippet-store/` | SQLite source of truth, migrations, generated engine YAML | GPL-3.0-or-later |
| Sync protocol types | `src-tauri/crates/sync-proto/` | Versioned wire records, validation, LWW semantics | Apache-2.0 |
| Sync client | `src-tauri/crates/sync-client/` | OIDC/static-token auth, pull/push loop, offline queue | GPL-3.0-or-later |
| Expansion engine | `espanso/` | Native text detection and injection | GPL-3.0 |
| Frontend | `ui/` | Webview screens and canvas editing tools | GPL-3.0-or-later |
| Reference server | `server/` | Authenticated, authorized sync and administration API | Apache-2.0 |
| Reference infrastructure | `infra/` | Optional AWS deployment | Apache-2.0 |

## Data and trust boundaries

### Local source of truth

SQLite is authoritative for snippets and their group metadata. The espanso YAML files are
generated atomically and are disposable. The engine only reads this generated projection; users
and application code must not treat it as editable state.

Capture metadata lives in SQLite and image data lives on disk under Glyphio's application data
directory. Clipboard history is device-local. History and clipboard content have no sync path.

### Expansion engine

The bundled engine is a thin, headless espanso fork run as a supervised sidecar. This preserves
the upstream process model and keeps changes rebasing-friendly. Glyphio owns the user interface,
tray, notifications, and configuration generation. The sidecar receives isolated
`ESPANSO_*_DIR` paths and hot-reloads generated configuration.

The fork has three deliberate deviations from upstream:

- On macOS it suppresses espanso's own Accessibility prompt because Glyphio owns permission
  guidance, and it rechecks trust periodically so granting access can restart the worker and
  recreate its event tap without restarting the app. Keep those changes local to
  `espanso/espanso/src/cli/daemon/mod.rs`.
- App-specific configuration files are read in file-name order. The first matching config still
  owns behavioral settings such as the injection backend, which lets generated
  `00_delivery_*` files give a bundle identifier an explicit paste/keys policy.
- `ConfigStore::active_match_paths` unions the match files from the default config and every
  app-specific config that matches the focused application. Upstream couples snippets to the
  single active config; Glyphio separates those concerns so a delivery-only config can win
  backend precedence without hiding snippets contributed by another matching scope.

The latter two deviations live in `espanso-config` and the worker config adapter. Keep the
first-match behavior and all-match-path union covered together: changing either independently can
silently select the wrong delivery policy or make an application's snippets disappear.

### Optional sync

The sync client serializes only team-scoped groups and snippets belonging to teams the identity
may access. It uses a pull → push → pull cycle, per-team cursors, durable dirty state, and
last-write-wins conflict resolution. OIDC uses Authorization Code with PKCE; static API tokens
are supported for simple deployments. Secrets are stored in the operating-system keychain, not
in SQLite or `sync.toml`.

The server validates the caller and team authorization on every request and stamps record
ownership. It sees deliberately shared snippet content in plaintext. The complete compatibility
contract is [Sync protocol v1](SYNC-PROTOCOL.md); deployment and operational guidance is in
[SETUP.md](../SETUP.md).

### Capture and permissions

On macOS, ScreenCaptureKit performs display and window capture. Accessibility permission enables
text expansion; Screen Recording permission enables capture. The UI and capture/editor flow are
native-shell orchestration plus a webview canvas editor. Redaction is an editing feature, not an
encryption boundary: users must redact before sharing a capture outside the device.

### Performance measurement

Every capture logs one line from `capture/trace.rs`:
`capture-trace mode=visible acquire=120ms encode=88ms present=4ms total=212ms size=5120x2880`.
It holds a mode name, fixed stage names and numbers only; the formatter takes no free text, so
titles, URLs and clipboard content cannot reach it. `acquire` includes the user's selection time
for picker and scrolling modes. Persist is traced natively (`Stage::Persist`). Launch logs
`launch-timing state-built|tray-ready|stores-open|engine-spawned|setup-done=<ms>` from
`launch.rs`. A launch renders the engine config only when its inputs changed
(`SnippetStore::render_yaml_if_changed`, with a digest in `.glyphio-render-stamp`), opens History
and the clipboard store lazily (or off the main thread once the menu bar is up), and a login
launch after setup builds no web view at all. `examples/bench_encode.rs` (synthetic 5K frame) and `examples/bench_capture.rs`
(`visible`, `frontWindow` through the real backend) print percentiles.

### Engine supervision

`engine::Supervisor` runs the daemon and records its pid in `<runtime>/glyphio-engine.pid`. Before
starting, it terminates the recorded process (and its worker child) only if it is alive and its
executable path is this bundle's `glyphio-engine` (`engine/reaper.rs`, over a `ProcessControl`
trait with a fake in tests). It never matches by name, so a development build beside a release
build never kills the other's engine. It waits for the daemon's `espanso-daemon.lock` only when
something was actually killed, with a 3 s bound. `start` runs in a spawned blocking task, not in
the main-thread `setup`. `engine/budget.rs` allows 5 respawns, refilled after 10 minutes of healthy
running; when spent, the engine stays stopped, the tray tooltip says so, Settings shows a banner
with **Restart engine**, and tray **Reload** also restarts it.

### Platform module

`src-tauri/src/platform/` is the only place for `cfg(target_os)` and OS crates. One trait per
capability (`ScreenCapturer`, `RegionPicker`, `WindowQuery`, `PageInspector`, `InputInjector`,
`Pasteboard`, `Permissions`, `Autostart`, `AppShell`, `ProcessControl`) with the macOS
implementation in `platform/macos/` (the former `capture/backend.rs`, `capture/ax.rs`,
`clipboard/platform.rs`, the Quartz and permission calls from `scroll.rs` and `commands.rs`, the
login item, the window-shell calls from `windows.rs`, and process control). `platform::current()`
builds the implementation once; `platform::fake` (tests) is a minimal in-memory one. Pure cores
(capture lifecycle, stitching, delivery rules, the engine reaper and respawn budget, the login-item
decisions) do not reference OS crates. Moved without behaviour change; the macOS verification
matrix is the check.

### Permission model

`permissions.rs` (platform-neutral, over `platform::Permissions`) maps what the system allows to one of
`Granted`, `NotDetermined`, `Denied`, `Stale` or `NeedsRelaunch` per permission, plus `expansion_live`
from the engine as a separate line. Accessibility comes from the app's own trust check (TCC credits the
bundled engine to Glyphio). Screen Recording is the preflight, then a real content query, which runs
only after the user was prompted once because it can prompt on a fresh system. "Prompted" lives in
`settings.json` (`permission_prompted_*`), imported once from the old web-storage flags by
`permission_import_legacy_flags`. `Stale` means the user says it is on (button, or 20 s after
returning from System Settings) and the system still refuses, or the bundle path differs from the one
recorded when it was last granted. A watcher polls once a second only while the UI has called
`permission_watch(true)` (setup or General settings visible), emits `permission-status` on change, and
restarts the engine when Accessibility becomes granted. `permission_repair` runs `tccutil reset` for
Glyphio's own entry only, then requests again. The old per-permission commands remain until the UI
moves to this model (ONB-02).

### Setup window

`windows::open_setup` shows `ui/setup/` (560×640, fixed, built through `summoned`) on launch while
`onboardingCompleted` is false; it replaces the Settings window on first launch (`startup::launch_surface`),
and a login launch still shows nothing. A `settings.json` written before the key existed counts as
completed, so existing installs skip setup. Steps (pure logic in `ui/setup/steps.mjs`, row presentation
in `ui/settings/permissions.mjs`): Accessibility, Screen Recording, Shortcuts, Finish; a small platform
table in `steps.mjs` (`stepsFor`, `loginItemCopy`) gives Windows only Shortcuts and Finish. The window turns on the
permission watcher while open, shows each native grant's single remedy (prompt, open System Settings,
**Fix it** for a stale grant, **Relaunch to finish** only when needed), comes forward when a grant lands,
and announces changes in a live region. `onboardingStep` and `onboardingSkipped` persist so a relaunch
resumes on the same step and moves on by itself if that permission is now granted.

**Shortcuts** lists the capture, palette and clipboard shortcuts with the registration result from
`shortcut_status` (`shortcutRows`). The **try-it** line completes only on the `capture-delivered` event,
which `tray::capture_delivery_finished` emits when a capture reached the editor or History, so a shortcut
that merely fired, or a capture that failed, does not count (`reduceTryIt`). **Launch at login** is a
switch, on by default and explained in the platform's words; nothing is registered until the user presses
Continue, which calls `onboarding_set_login_item`. Until setup is finished `startup::may_register_login_item`
stops launch from registering the item, so an installation that quits mid-setup has still not been
registered. **Finish** offers just me, I have an invite and set up team sync (`teamAction`); the last two
call `onboarding_open_team`, which opens Settings and sends `show-sync`, or the same `invite-link` event
the system's link handler sends, so a pasted invite is confirmed in Settings and never applied from here.
Setup sets `glyphio-welcomed` so Settings does not show its own welcome as well.

### Login item

"Launch Glyphio at login" is `SMAppService.mainApp` (`platform/macos/autostart.rs`), the system's own login-item
service: it appears in System Settings → General → Login Items, needs no Automation permission and
writes no LaunchAgent plist. The decisions are pure functions over a `LoginItem` trait, tested with
a fake. The item is changed only when the user toggles the setting; launch reads its status and
registers once if the setting is on and nothing is registered. A registration awaiting approval, or
refused by the system, is shown under the toggle in Settings with the system's wording and is never
retried silently. A login launch is recognised from the launch Apple event
(`keyAELaunchedAsLogInItem`, read in `startup::note_launch_event`), and for one more release also
from `--hidden` / `--launch-at-login`, so it shows no Settings window. On first launch of this
version `autostart::migrate_legacy` removes the AppleScript login item named "Glyphio" (the only
remaining `osascript` call, best-effort, and skipped on a fresh install so a new user is never asked for Automation; if Automation is denied Settings says to remove it by hand)
and any LaunchAgent plist whose first program argument is `…/Contents/MacOS/glyphio` inside a
bundle with identifier `io.glyphio.app`, wherever it lives. `scripts/verify-edr-surface.sh` asserts
no scripted backend is selected and `osascript` appears nowhere else. There is no login-item
backend off macOS yet (see `docs/WINDOWS.md`).

### Clearing clipboard history from the palette

The palette's clipboard list can clear history with an inline two-key confirmation (a dialog would
take focus, and the palette closes on blur). `ui/palette/clip-keys.mjs` maps keys to intents.
`clear_clips(include_pinned, empty_pasteboard)` clears unpinned entries only when asked to
(`ClipStore::clear_unpinned`), or everything (`clear`, which Settings still uses; omitting the
argument means "including pinned"). Image files of removed entries are deleted either way, and
pinned images stay. Emptying the system clipboard goes through `clipboard::empty`, which marks the
change as our own so the watcher does not record it; the choice is stored as
`clipboardClearAlsoEmptiesPasteboard`.

### Shortcut registry

`shortcuts::registry::Registry` (in `AppState`) owns the table of bindings (a capture mode with
its silent twin, open history, open palette, open clipboard), the chords the system has accepted,
and how each binding fared. `apply` diffs the settings against what is held: only removed or
changed chords are unregistered and only new ones registered, so saving an unrelated setting makes
no register call and never touches Escape, which a scrolling capture borrows separately. A chord
the system refused is asked for again on the next save, since whatever held it may have let go.
The key handler resolves a press by lookup, not by parsing accelerator strings, and ignores the
same action again within 250 ms. If two bindings share a chord, the first listed wins and the
other is reported as unusable. `save_settings` returns each binding's result and `shortcut_status`
returns them on demand; Settings shows "Not registered" with the reason beside any refused or
invalid binding, and the tray tooltip and status row say "Some shortcuts could not be registered
— open Settings" while the app is otherwise idle.

Reserved chords are per-platform data in `platform/reserved.rs` (`reserved_for(Os)`), used by
`validate_settings`; `ui/settings/shortcut-recorder.mjs` keeps the matching table for the recorder
(`validateAccelerator(accelerator, configured, platform)`). On Windows `Alt+Space` (the window menu),
`Alt+Tab`, `Alt+F4`, `Ctrl+Alt+Delete`, `Ctrl+Shift+Esc` and common Win chords are reserved; macOS keeps
Spotlight, screenshots, lock and Force Quit. The palette default is `Control+Shift+Space` on both. Existing
installs keep the chord they already have stored (including the old `Alt+Space`): the default only applies
to a settings file that has no palette shortcut, so nothing changes silently. In the setup window a
refused or invalid default shows the system's reason, up to three ordered alternatives
(`ui/settings/shortcut-defaults.mjs`, filtered by the same validation) and **Record another**; choosing one
saves it through `save_settings` and shows the new result.

### Capture worker

Captures run on one dedicated thread (`capture::worker`), never on the main thread. A hotkey,
the palette, the tray or the `trigger_capture` command only begins the lifecycle and queues a
request (`capture::trigger`), then returns. The worker runs the interactive picker, the browser
accessibility lookup, the scroll loop and PNG encoding, and hops to the main thread
(`capture::on_main`) only to create windows, waiting there itself so the main thread never waits
on it. While the region picker is open the tray menu and Settings therefore stay live. Requests
run one at a time; a second capture is already refused by the lifecycle, and the single scrolling
capture flag is unchanged. A panicking request is logged and the worker carries on; the capture
deadline below clears its indicator.

### Scroll progress oracle

The scrolling loop (`capture::scroll`) can ask its `ScrollDriver` for a `Progress` (offset, extent,
viewport in points) that does not come from the pixels. The production driver asks
`PageInspector::scroll_progress` at the region centre, where the wheel events land. On macOS
(`platform/macos/inspect.rs`) that walks up from the accessibility element under the point: a
page's `AXWebArea` wins wherever it sits in the chain (its frame against the window gives the
offset, so a nested scroller inside the page moves nothing), otherwise the nearest `AXScrollArea`
answers from its vertical scroll bar. The owning browser's tree is kept warm on every ask and
released by the existing cooldown. Everywhere else, or when nothing answers, it is `None` and the
loop uses pixel comparison alone. The arithmetic is pure (`platform::types::Progress`). For now
the loop only logs whether the page advanced; stop reasons use it in SCROLL-03.

### Scroll stop reasons and settling

A scrolling capture ends for one of six logged reasons (`scroll::StopReason`): `ReachedEnd` (the
oracle says the viewport is at the end, or, without an oracle, nothing changed after two retries
that wait 600 ms and 1.2 s), `Stopped` (Escape), `PointerLeft` (the 30 s pause limit), `LimitReached`
(page height gathered reached `scrollMaxHeightPt`, default 30 000 pt), `Stalled` (the oracle says
more remains but three scrolls moved nothing) and `Resized` (a frame came back at another size, so
its rows cannot join the image; the frames before it are kept). After each scroll the loop settles by reading frames
80 ms apart until two consecutive reads match, up to about 1.5 s; an animation that never settles
uses the last read and the oracle decides. An oracle that already reports the end before the first scroll describes the viewport, not the page (some browsers expose no document extent), so it is ignored for that capture and the pixels decide. When an oracle exists a frame is kept only if the offset
advanced, so a nested scroller that changes pixels is dropped. Each read calls the capture heartbeat.
Progress ("40%" with an oracle, else a frame count) is written to the active capture's menu text, and
a capture that ended before the end of the page (limit, stall, pointer) carries the reason into its
Succeeded presentation. The loop talks to its environment through `LoopControl`, so tests run with
virtual time. The setting has no UI yet; it is read from `settings.json`.

### Capture delivery

`capture::finish` encodes once and then saves the capture natively (`persist_native`) before any
window opens: the PNG, a 320 px thumbnail and the history row. With history switched off the row
is a scratch row in an owner-only `history/scratch` folder, left out of the list and of retention,
and removed at launch, when the live editor closes, and when the next capture replaces it. The
editor and the silent worker then receive the row id and file path, load the image through the
asset protocol, and only add the banner, copy and acknowledge, so killing the page mid-delivery
still leaves the capture in history. The banner strip is drawn natively (`capture::banner`, with
the bundled Inter font) from the same line rules as `ui/shared/banner.js`; the editor asks for the
strip with `render_banner_strip`, and a silent capture never loads a page at all: the strip is
stacked on the stored PNG and written to the pasteboard (PNG plus TIFF) by `capture::finish`.
Golden images for each banner setting at 1x and 2x live in `src-tauri/tests/golden/banner`
(`GLYPHIO_BLESS=1 cargo test banner` regenerates them). The pre-1.3 path, where the page encodes and saves itself,
stays for one release behind the `legacyDelivery` setting.

What follows a capture is the `captureDelivery` setting: `preview` (the default), `editor` or
`silent`. A file without it is migrated from the old `silentCapture` switch (silent stays silent,
everything else becomes the preview), and saving writes `silentCapture` back in step for an older
build. A preview capture is copied exactly like a silent one, then `windows::open_preview` builds
`ui/preview` per capture as a summoned surface, never focused, in the bottom-right corner of the
active display, so it appears over full-screen apps without taking the keyboard. Its Edit opens the
row in the editor, Discard deletes the row (the clipboard keeps what was copied), and the card
closes itself after a few seconds unless the pointer is on it (`ui/preview/state.mjs`). Scratch
rows are purged once no editor, preview or stored-capture window is left showing one. With
`legacyDelivery` on, the preview falls back to the editor.

Image bytes cross the IPC boundary raw, never as base64: the page sends the PNG as the invoke
body and the other arguments as percent-encoded JSON in the `x-glyphio-args` header
(`ui/shared/ipc-bytes.mjs` and `src-tauri/src/ipc_bytes.rs`), and Rust answers with a raw
`Response` (`read_capture_png`). Every command that reads or writes image or file bytes is async
and does the work in `spawn_blocking`, thumbnails are made natively from the content PNG
(`capture::thumbnail_from_png`), and History's Copy and Save composite the banner natively
(`copy_stored_capture`, `export_stored_capture`). In debug builds `watchdog.rs` posts a no-op to
the main thread every 100 ms and logs any that run more than 250 ms late, naming the last command
invoked and a tally of the commands that preceded stalls.

### Scroll stitching

`capture::scroll::stitch` is pure (frames in, rows out). Its `Stitcher` joins frames as the loop
takes them and writes each frame's fresh rows straight to `scroll::png_stream`, a PNG encoder fed a
row at a time (filtered per row, deflated as it goes, the header written at the end once the
height is known). It holds the last frame written and the newest one, which waits to learn
whether it is the final frame; the result never exists as RGBA, and `Shot::png` carries it to
delivery, which skips its own encode. A 1 600 × 60 000 px capture (800 pt by 30 000 pt at 2x)
peaks at about 90 MB against 366 MB for the raw image (`tests/scroll_memory.rs`). With three or
more frames it finds rows at the top and bottom that are identical in the first three frames
while the middle moved (a fixed header or
composer bar; flat bands are ignored) and columns that never change (a fixed sidebar). The header is
kept from the first frame and the footer from the last; the rest are cropped, and bands and
sidebar columns are left out of the overlap signature. Each join then searches for the overlap:
when the progress oracle measured the scroll distance, within ±4 px of it; otherwise within ±90 px
of the nominal distance, and over the full range for the final frame (whose scroll is usually
partial) or whenever the narrow window has no confident match. The full range is a coarse pass on
every 4th row and offset followed by refinement of the six lowest coarse differences, with ties
settled toward the expected overlap so repeated content does not pull the join. A probe whose rows
are blank (paragraph spacing, a margin) matches anywhere, so it moves up to the lowest band with
something in it, by at most a quarter of the frame. The loop records the oracle's offset change
between kept frames for this. The stitching tests use synthetic pages drawn like real ones
(`scroll/fixtures.rs`: an article with text, code, pictures and blank runs, a feed of cards that
share one layout, chat chrome with a header, composer and sidebar).

Every frame of a capture comes from one capture session (`ScreenCapturer::rect_session`). On
macOS that is a `RectPlan`: the shareable content, the displays the region falls on, our windows to
leave out and each display's filter and configuration are settled once, and each frame is only
the screenshot call. A frame that fails begins a new session once before the error stands, so a
display rearranged mid-capture is picked up. Windows Glyphio opens after the plan is made are not
excluded; none open during a scrolling capture.

### Capture deadlines

A capture cannot block the next one indefinitely. `capture::lifecycle` gives every active capture
a deadline: 150 s for modes that include a selection (snip, window picker, scrolling area), 30 s for
the rest, 20 s once pixels exist and delivery has begun. Scrolling frames call a heartbeat that
pushes the deadline 20 s ahead, capped at 5 minutes from the start. A watchdog in `tray.rs` checks
by capture token; an expired capture becomes `Failed` (logged at error level with mode and phase)
and a stale watchdog never touches a newer capture. Tray **Reload** resets the lifecycle to idle.

Backing out is not failing. A cancelled interactive picker surfaces as `capture::Cancelled` and
clears the indicator for its own capture token. The scrolling selection overlay carries a
`selecting` flag in the lifecycle: if the overlay is destroyed, blurred or hidden before it
reports a rectangle, the capture is cancelled; closing it to start a capture is not. Each overlay
has its own label so a replaced overlay's destruction cannot cancel its successor.

### Single instance

Exactly one `glyphio` process runs per user. `tauri-plugin-single-instance` is registered first
in the builder; a second launch (Finder, direct execution, a `glyphio://` link) forwards its
arguments to the running copy over a socket and exits during builder startup, before a tray
icon, hotkey, engine or window exists. The running copy brings Settings forward unless the launch carried the login marker
(`startup::second_launch_action`). `relaunch_app` and the updater go through `app_restart`, which
releases the socket before `restart()` so the replacement process claims the lock rather than
handing its launch to the process that is exiting. The pid and executable path are logged at
startup. `glyphio-engine` is intentionally two processes (daemon and worker); that is not a
duplicate instance.

## Architectural invariants

1. Local-first is the default: configuration, personal snippets, captures, and clipboard history
   stay on the device.
2. Sync is opt-in and team-scoped. Command snippets and executable variables never sync.
3. The server is replaceable. Client/server compatibility is defined by the versioned protocol,
   not by the reference implementation.
4. The espanso fork stays thin. Product behavior belongs in the Tauri application, frontend, or
   generated configuration unless an engine change is unavoidable.
5. Security-sensitive authorization is enforced on the server; client-side controls improve UX
   but are not the trust boundary.
6. Protocol changes are additive within `/v1/`; incompatible changes require a new API version.

## Key decisions

| Decision | Rationale | Consequence |
| --- | --- | --- |
| Supervise espanso as a sidecar | It matches espanso's daemon model and limits fork drift | The shell owns process lifecycle and config generation |
| SQLite + generated YAML | Gives Glyphio rich local state while retaining espanso compatibility | YAML is not user-editable source data |
| Protocol-compatible BYO backend | Keeps self-hosting and enterprise deployment portable | Wire changes need compatibility discipline |
| Team-only sync | Reduces accidental disclosure and isolates personal data | Sharing requires an explicit group action |
| Server-side RBAC and restricted groups | Clients cannot reliably enforce collaboration permissions | Reference server remains a security-critical component |

## Verification responsibilities

- Unit and integration tests cover stores, sync types/client, and server behavior.
- GUI and permission-gated flows require a macOS verification session before release.
- Any protocol, authorization, secret-storage, or capture-data change requires review against
  [Security posture](SECURITY.md) and the relevant spec.
