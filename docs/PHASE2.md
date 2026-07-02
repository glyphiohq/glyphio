# Glyphio — Phase 2 build notes (2026-07-02)

What was built on top of Phase 1 (`PHASE1.md`), per `PROMPT-PHASE2.md` and the approved
`PHASE2-PLAN.md`. Headline: **Glyphio is now open-source-first** — pluggable sync/auth, a
documented wire protocol, a self-hostable reference backend, and the reference enterprise deployment Okta+AWS reduced to
one configuration of the generic machinery.

## Licensing (decided with the human)
- **App = GPL-3.0-or-later** (distributing the espanso fork triggers copyleft; Checkpoint
  code is the author's own IP, relicensed in).
- **`sync-proto`, `server/`, `infra/` = Apache-2.0** so third parties can implement the
  protocol without copyleft obligations. See `LICENSE`, `NOTICES.md`.
- Still open for an enterprise production rollout: OSS/legal sign-off, prod Okta app, code signing.

## Sync architecture (client)
- `crates/sync-proto` — wire types + LWW rule + limits; the executable form of
  `docs/SYNC-PROTOCOL.md`.
- `crates/sync-client`:
  - `auth`: `AuthProvider` trait → **OidcAuth** (generic Authorization Code + PKCE S256,
    loopback redirect per RFC 8252, `state`/`nonce` checked, ID-token JWKS validation via
    `openidconnect`; silent refresh; bearer = ID token, portable across IdPs) and
    **StaticTokenAuth** (pasted token). Secrets only in the OS keychain
    (`io.glyphio.sync`).
  - `http`: `SyncProvider` trait → `HttpSync` (reqwest/rustls). Alternative backends =
    new impl, no engine change.
  - `engine`: pull → push → pull per team; LWW applied through the store's
    `apply_remote_*`; dirty-flags are the offline queue; debounced change-listener pushes;
    exponential backoff; **team-scope enforced at the query level** (personal snippets and
    history have no code path to the wire).
- `snippet-store` migration **v3** (additive): `groups.team`, `sync_cursors`, `sync_pushed`;
  `ChangeEvent` gained `entity` + `origin`. Migration runner hardened against **mixed binary
  versions** sharing one data dir (never downgrades the stamp; tolerates re-run ALTERs) —
  found live when the Phase-1 DMG app and the dev build both touched the DB.
- Tauri: `src/sync.rs` (engine lifecycle + commands), Settings → **Team sync** UI
  (status card, OIDC/token config, sign in/out, Sync now), per-group **⇅ share-with-team**
  action (`set_group_team` cascades to member snippets) + team badges.
- Config: `sync.toml` in the app data dir (no secrets; `config.example.toml` documents it).
  **Sync is off by default — a fresh build makes zero network calls.**

## Reference backend (`server/`, Apache-2.0)
axum; auth = static tokens (SHA-256, constant-time) and/or generic OIDC JWT validation
(JWKS discovery + cache); per-team authorization on every route; server stamps `owner`
from the authenticated sub; validation per protocol limits; per-credential rate limiting;
RFC-7807 errors; storage trait with **SQLite** (self-host default, docker-compose) and
**DynamoDB** (single-table + `by-seq` GSI) selected at runtime. `infra/` = Terraform for
Lambda (container image + web adapter) + HTTP API (throttled) + DynamoDB + least-privilege
IAM — all values parameterized, zero deployment-specific defaults.

## Phase 1 deferred items — all closed
See the updated Deferred section in `PHASE1.md`: app_scope enforcement (per-scope espanso
app configs), real capture DPR (+ fixed a 2× Retina downscale bug in SCK visible capture),
snip = native interactive picker (dead DOM-overlay port removed), Screen Recording
onboarding banner, real icon (`scripts/gen-icon.swift`). Bonus fix: the snippet editor was
silently wiping `appScope`/`team` on every save.

## UI design (final direction)
Per a mid-phase directive, the UI is **brand-neutral** — an earlier branded pass was
replaced by the **"Ink & Brass"** system (`ui/shared/theme.css`): near-black ink surfaces,
hairline strokes, one warm brass accent (#D9A54A), system typography (SF Pro / SF Mono with
bundled Inter / Roboto Mono as non-macOS fallbacks — no network fonts), a monospace voice
reserved for glyph-things (triggers, kbd, micro-labels), and a blinking brass caret as the
wordmark's only flourish. All stylesheets consume the token layer, so future retheming is a
one-file change. Icon matches (`scripts/gen-icon.swift`).

## Team roster & membership
`GET /v1/teams/{team}/members` (additive, optional) surfaces the roster: configured members
in static-token mode, "seen members" in OIDC mode. The Settings sync card shows team chips +
searchable member lists; "Add member…" opens mode-aware guidance. **Membership deliberately
has no write API** — the IdP (or the server's token config) stays the single source of truth.

## Verification status
**Machine-verified this phase:**
- 17 client-side tests green (store 10, sync-client 5, sync-proto 2) incl. a two-device
  convergence test vs a protocol-faithful fake.
- Server: 6/6 tests + live HTTP smoke (401/403/owner-stamp/cursor roundtrip).
- **Full-stack E2E over real HTTP**: two real stores + two real engines + the real server —
  team snippets converge, personal snippets never leave, LWW conflict resolution, tombstones
  (`cargo test -p sync-client --test e2e_http -- --ignored` with a running server).
- espanso parses the generated scoped app configs (`glyphio-engine match list`).
- App builds and boots clean; sync disabled ⇒ no network; migration self-heal on the real DB.

**Needs a human GUI session (permission-gated):**
- Live typing-expansion incl. a scoped snippet in/out of its target app.
- Real captures on Retina (banner sizing with the new DPR) + editor tools + history.
- OIDC sign-in flow against a real IdP (Okta Preview) — browser + loopback round-trip.
- The Team-sync UI flow end-to-end (sign in → share group → second device).
- Terraform apply to sandbox AWS (CLI not installed here; DynamoDB path is
  compile/test-verified but not exercised against real AWS).

**Dev-environment notes (macOS TCC — read before debugging "permissions don't work"):**
- `scripts/dev-sign.sh` signs **both** the engine sidecar and the app binary with the stable
  "Glyphio Dev" identity, so Accessibility/Screen-Recording/keychain grants survive rebuilds.
- **Screen Recording only registers for a real `.app` bundle launched normally** (Finder /
  `open`). A bare `target/debug/glyphio` run from a terminal gets TCC-attributed to the
  *terminal*, so "Enable screen capture" appears to do nothing and no Glyphio row shows up in
  System Settings. For capture testing use the bundle (`npm run build` →
  `src-tauri/target/release/bundle/macos/Glyphio.app`, install to /Applications). Text
  expansion is unaffected (the engine is its own signed process either way).
- Stale/duplicate TCC rows from older builds: `tccutil reset ScreenCapture
  io.glyphio.app` clears them, then grant once for the installed bundle.
