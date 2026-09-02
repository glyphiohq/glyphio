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

The fork has two macOS Accessibility deviations. It suppresses espanso's own permission prompt
because Glyphio owns permission guidance, and it rechecks trust periodically so granting access
can restart the worker and recreate its event tap without restarting the app. Keep both changes
local to `espanso/espanso/src/cli/daemon/mod.rs`.

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
