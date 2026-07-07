# Glyphio — Phase 5: interactive snippets, secure commands, capture/history upgrades, admin dashboard v2

What was built, per the approved Phase 5 plan. Owner decisions honored throughout:
**command snippets never sync** (local-only, not approval-gated); **browser capture =
frontmost window bounds** (no companion extension); **admin dashboard fully revamped
including browser OIDC sign-in**.

## A. Rich editor: Rich ↔ HTML source toggle

`format: html` snippets gain a small **[Rich | HTML]** segmented switch above the editor —
WYSIWYG contenteditable on one side, raw HTML source in a mono textarea on the other,
content synced both ways, live preview follows either. (`ui/settings/settings.js`)

## B. Interactive snippet kinds + images

* **Data model (migration v4):** `snippets.kind` (`text | form | popup | command`) and
  `snippets.enabled` (quarantine/off flag). Wire protocol gains an additive optional
  `kind` (absent = `text`; `command` is not a legal wire value).
* **Engine bridge:** the fork gains one deviation — a `glyphio` render extension
  (`espanso-render/src/extension/glyphio.rs`) handling `glyphio_form` / `glyphio_popup`
  vars over a user-only unix socket (`GLYPHIO_IPC_SOCKET`, exported by the app at engine
  spawn). App side: `src-tauri/src/bridge.rs` (tokio listener; forms block the expansion
  until submit/cancel with a 110 s timeout; only live + enabled snippets resolve).
* **Popup kind** — typing the trigger opens an always-on-top Tauri window (`ui/popup/`)
  rendering the snippet body (plain/markdown/html incl. images); the trigger text just
  disappears. The cheatsheet surface.
* **Form kind** — typing the trigger opens a form window (`ui/form/`); fields come from a
  visual builder in the snippet editor (text / multiline / choices), or fall back to one
  text input per `{{placeholder}}`. Submit fills the body template and the engine pastes
  it in the snippet's format; Esc/close aborts the expansion cleanly.
* **Images in snippets** — Insert-image toolbar button + paste-into-editor: downscaled
  (≤ 1600 px, ~≤ 300 KB) and embedded as a data URI in the HTML body, so images sync,
  export, render in popups/forms, and paste with rich content. `MAX_REPLACEMENT` raised
  100 KiB → 1 MiB, `MAX_BODY` 1 MiB → 8 MiB.
* **Sanitization** — snippet HTML is sanitized (`ui/shared/markdown.js`) before touching
  any app webview (preview / popup / form): team-synced markup is untrusted.

## C. Command snippets — local-only, defense in depth

`kind: command` runs a shell command (sh/bash/zsh picker) and pastes its output. The
pre-existing hole — espanso `shell`/`script` vars syncing unfiltered — is closed at every
layer:

| layer | enforcement |
|---|---|
| store | `kind=command` ⇒ `team = NULL` always (create/update/group-cascade) |
| YAML render | disabled snippets and team'd commands never reach the engine |
| client push | command kind + exec vars excluded from batches |
| server | push validation rejects `kind=command` and `shell`/`script` vars (422) |
| client pull | exec vars stripped, record applied **disabled** (rogue-server defense) |
| import | JSON/YAML with executable content arrives disabled, "off — review" badge, explicit confirm-to-enable |

## D. Capture: frontmost-window mode

New non-interactive `frontWindow` mode (hotkey default ⌥⇧W, tray item, settings toggle):
captures the frontmost window's full bounds via ScreenCaptureKit — one keystroke for "just
the browser window, not the whole screen". The scroll-capture title-bar inset became a
parameter (`frontmost_window_bounds_with_inset`).

## E. History: content/banner split, immutable timestamps, easy crop

* History now stores the **content-only** PNG plus banner metadata (`note`,
  `banner_enabled`, legacy `banner_baked`); the banner is composited at view/copy/export
  time by shared code (`ui/shared/banner.js`, also used by the history list). The banner
  timestamp always renders from the immutable original `captured_at`.
* History entries are **editable**: toggle the banner on/off, edit the note, re-crop,
  redact/draw/text — persisted in place via the new `update_capture` command. Rows saved
  before the split stay view-only.
* Crop UX: corner drag-handles to resize, drag inside to move, Enter applies.
* Edits (crop/redact/draw/text) now land on the content canvas — the banner/note stay
  editable afterwards instead of locking.

## F. Admin dashboard v2 (server console)

* **Sign-in:** browser-side OIDC Authorization Code + PKCE (state + nonce checked, token
  in tab session storage, server still re-validates every request) via the new public
  `GET /admin/v1/config` (`ADMIN_OIDC_CLIENT_ID` / `ADMIN_OIDC_SCOPES` env). Token paste
  remains as the static-token fallback. Sign-out button; 401 auto-signs-out.
* **Views:** Overview (stat tiles + 30-day activity bar chart with per-day breakdown
  tooltips, from the new `GET /admin/v1/stats`) · Teams (searchable list → team detail
  page: roster, roles, invites, restricted groups/ACL, archive) · Members (searchable
  cross-team table → member detail with per-team revoke) · Audit (team/action/actor
  filters — new `action`/`actor` query params — plus CSV export) · Org settings.
* Still a single static file served by the binary, zero external assets, Ink & Brass.

## Verification

`cargo test` green across `src-tauri` workspace (31 tests: store/yaml/portability/sync
quarantine) and `server/` (16 tests incl. exec-content rejection, stats, audit filters,
config endpoint). Engine fork builds `--no-default-features --features native-tls` with
the new extension. Server smoke-tested live: `/admin` serves the v2 console,
`/admin/v1/config` public, `/admin/v1/stats` role-gated with bucketed activity.

## Deferred

* Content-addressed asset store for snippet images (if snippets outgrow the 1 MiB cap).
* Browser-extension viewport capture (Phase 3 §C remains the reference design).
* espanso `form:`-match import (still skipped with a report on YAML import).
