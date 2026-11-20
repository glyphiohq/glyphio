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

### Login item

"Launch Glyphio at login" is `SMAppService.mainApp` (`autostart.rs`), the system's own login-item
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
