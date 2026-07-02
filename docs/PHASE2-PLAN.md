# Glyphio — Phase 2 plan (sync + auth + open-source hardening)

Tight plan per PROMPT-PHASE2.md §0. Check-in required before implementation.

## 0. Licensing (decision needed — recommendation below)
- Open-sourcing = distributing the espanso fork ⇒ GPL-3.0 copyleft applies to the combined work.
- **Recommendation: license the entire repo GPL-3.0-or-later.** Tauri (MIT/Apache-2.0) and all
  crates used are GPL-compatible; Checkpoint code is the owner's own IP and can be relicensed.
  The reference backend (`server/`) is a separate program communicating over HTTP — it could be
  MIT/Apache if we want maximal server adoption, but single-license GPL-3.0 is simpler. Proposal:
  **GPL-3.0 for the app, Apache-2.0 for `server/` + the protocol doc** (protocol implementations
  shouldn't be copyleft-constrained). Needs a yes.
- Keep `espanso/LICENSE` + upstream copyright notices untouched; add root `LICENSE`, `NOTICES`.
- Publishing also obligates: source availability for the shipped binaries (the public repo
  satisfies this). OSS/legal review recommended before public release (non-blocking).

## 1. Crate/module layout (new code)
```
src-tauri/crates/
  sync-proto/      # shared types: wire records, requests/responses, (de)serialization — no deps on app
  sync-client/     # AuthProvider trait + OidcPkce/StaticToken impls; SyncEngine; SyncProvider trait + HttpSync
server/            # reference backend: axum + storage trait (SQLite | DynamoDB); own workspace, NOT in app build
infra/             # Terraform for the AWS reference deploy (all values = variables)
```
`sync-proto` is shared by client and server so records can't drift.

## 2. Auth (§A)
- `AuthProvider` trait → `async fn credential() -> BearerToken`, `fn identity() -> Option<Identity>`
  (sub, email, teams), `sign_in/sign_out`, `status()`.
- **OidcPkce** impl: `openidconnect` crate (discovery, auth-code+PKCE S256, `state`+`nonce`, JWKS
  ID-token validation, silent refresh). Loopback redirect: one-shot `TcpListener` on
  `127.0.0.1:<ephemeral>`, opens system browser, serves a tiny "you can close this tab" page.
  Config: `issuer`, `client_id`, `scopes`, optional `audience` — zero provider-specific code.
- **StaticToken** impl: user pastes a token; stored in keychain; sent as bearer. No IdP needed.
- Tokens (access/refresh/ID) only in **macOS Keychain** (`keyring` crate, service
  `io.glyphio.sync`). Never in SQLite/settings.json/logs. Expiry/revocation ⇒ signed-out
  state + UI banner, engine pauses (no retry storm).
- Team membership: from a configurable ID-token claim (`teams_claim`, default `groups`); static-token
  mode gets teams from the server (`GET /v1/me`).

## 3. Wire protocol (§B1 — docs/SYNC-PROTOCOL.md)
Versioned REST+JSON, bearer auth, HTTPS. Server-assigned **per-team monotonic cursor** (client
clocks never order the stream):
- `GET  /v1/me` → identity + teams (drives UI + static-token team resolution).
- `GET  /v1/teams/{team}/changes?since=<cursor>&limit=N` → `{snippets:[…], groups:[…], next_cursor, more}`
- `POST /v1/teams/{team}/changes` → batch push `{snippets:[…], groups:[…]}`; server LWW-merges,
  returns `{results:[{id, accepted|superseded, server_record?}], cursor}` (superseded ⇒ server's
  newer copy comes back — client applies it; no separate conflict phase).
- Records = `sync-proto` shapes (snippet: id…deleted_at incl. format/group_id; group: id, name,
  sort_order, team, updated_at, version, deleted_at). Tombstones sync as records with `deleted_at`.
- Errors: RFC-7807 problem+json; 401 (bad/expired token), 403 (not your team), 413 (too large),
  422 (validation), 429 (Retry-After). Protocol version in the path (`/v1/`); additive fields allowed.

## 4. Sync engine (§B2)
- **Additive store changes** (migration v3, existing runner — no data risk):
  - `groups.team` column (groups are currently sync-columned but team-less; team snippets must be
    able to carry their folder).
  - `sync_meta` table: `last_cursor` per team + `pushed_version` per record (dirty = version >
    pushed_version). Store gains `apply_remote(record)` (verbatim upsert iff remote
    `(updated_at,version)` wins — LWW client-side too) and `dirty(team)` — both additive; existing
    CRUD/YAML generator untouched. `apply_remote` fires the normal `ChangeEvent` so YAML + UI refresh
    for free (with an origin flag so the engine doesn't re-push what it just pulled).
- `SyncEngine`: subscribes to `add_change_listener`; debounced push (2s), pull on login/interval
  (default 5 min)/manual "Sync now"; offline queue = the dirty flags themselves (nothing to lose);
  exponential backoff + jitter, cap 15 min.
- **Hard scope rule enforced in the engine**: only records with `team ∈ identity.teams` are ever
  serialized to the wire; `personal` snippets and ALL history/captures have no code path to the
  network. (History module is untouched by this phase.)
- `SyncProvider` trait (push/pull) with `HttpSync` impl; future file/S3/WebDAV = new impl only.
- Tauri: new `sync.rs` (state + commands: `sync_status`, `sign_in`, `sign_out`, `sync_now`,
  `get/set_sync_config`) + events (`sync-status`); Settings UI gets a Sync section (signed-in
  identity, last sync, errors, sign in/out, Sync now) reusing existing dark tokens.
- App sync config (issuer, client_id, backend URL, auth mode, teams_claim) lives in a
  `sync.toml` next to settings.json — runtime-editable, `config.example.toml` committed,
  real file gitignored. **Sync fully off until the user configures it** (guardrail).

## 5. Reference backend (§B3) — choice + justification
- **One axum (Rust) server with a `Storage` trait; two impls: SQLite (default, self-host) and
  DynamoDB (AWS reference).** Why: same language/types as the client via `sync-proto` (no drift);
  self-hosters get `docker run -v data:/data` and done; DynamoDB impl keeps the reference sandbox deploy
  serverless. Rejected: Lambda-only Node/Python (type drift, two codebases), Postgres (heavier
  than needed for snippet records).
- AWS reference deploy: **Lambda (container image + aws-lambda-web-adapter) + API Gateway HTTP API
  (throttling) + DynamoDB (single table, PK=team, SK=record; GSI on cursor)**. Near-zero idle cost,
  nothing to patch, HTTPS + rate-limit at the edge. `infra/` = small Terraform, every value
  (region, account, Okta issuer/audience, table name, throttle rates) a variable; no deployment-specific defaults.
- Server security: JWKS validation (issuer/audience from env) on every request; team authz =
  token's teams claim must contain `{team}`; static-token mode = argon2-hashed tokens from env/file
  with a `token→teams` map; body-size limits (1 MiB), per-record size caps, serde-strict
  validation; rate limit (tower middleware + APIGW throttle); structured logs with **no token/body
  logging**; least-privilege IAM (Lambda→ its table only). Trust model documented in
  SYNC-PROTOCOL.md (server sees team snippet content in plaintext — E2E encryption noted as
  future work, out of scope).

## 6. Deferred Phase 1 fixes (§D)
- **Snip overlay**: borderless fullscreen transparent always-on-top window on the cursor's display
  (`ui/snip-overlay/` already ported) → drag rect → `capture_region(rect)` via
  `SCScreenshotManager::capture_image_in_rect` → editor. Esc cancels.
- **app_scope enforcement**: YAML generator emits, per distinct scope, `match/_scoped_<scope>.yml`
  (underscore = not auto-included) + `config/app_<scope>.yml` with `filter_exec`/`filter_title`
  and `extra_includes`. Scope string format: `exec:<bundle-or-exe>` | `title:<substr>` (documented).
  Generator-only change; espanso engine untouched.
- **Capture DPR**: return the display's backing scale (CGDisplay pixel/point ratio) with each
  capture; editor already consumes `dpr`.
- **First-run permissions UI**: onboarding pane in Settings (shown until both grants OK):
  Accessibility (existing status plumbing) + Screen Recording (`CGPreflightScreenCaptureAccess` /
  request), with "Open System Settings" buttons.
- **Icon**: generate a real Glyphio glyph-mark icon set (neutral default for OSS; the reference enterprise deployment branding only
  in the reference deployment docs, not baked in).
- Fix warnings/bugs encountered; espanso fork diff stays as-is (no new engine edits expected).

## 7. Docs & hygiene (§C, §E)
- `SETUP.md`: generic self-hoster path (any OIDC IdP: public client, auth-code+PKCE, loopback
  redirect URI, scopes; any protocol-compliant backend; static-token path), Enterprise reference path (example: Okta + AWS)
  (where to paste Okta Preview issuer/client_id + backend URL **after build**; Terraform deploy
  steps), secrets checklist (what lives in keychain / env / TF vars; nothing committed).
  Placeholders only (`<OKTA_ISSUER>` etc.).
- `README.md` (what/why, quick-start, self-host model, links), `CONTRIBUTING.md`,
  `docs/SECURITY.md` (threat model, synced-vs-local table, secret locations, vuln reporting),
  root `LICENSE` + `NOTICES`, `server/.env.example`, `config.example.toml`.
- Verify `.gitignore` (real config/env, keychains, target/, binaries/, dist/) and `git status`
  clean of secrets. No telemetry/auto-update/third-party calls — only the user's configured
  IdP + backend, off by default.

## 8. Order of work & verification
1. §D fixes (small, independent) → 2. `sync-proto` + store migration v3 → 3. auth → 4. sync engine
→ 5. server (run locally: `cargo run` + SQLite) → 6. **E2E: two local store instances syncing
through the local server, LWW + tombstones + personal-never-syncs asserted in an integration
test** → 7. Terraform (validated with `terraform validate`/plan-level only; apply happens when the
owner supplies sandbox creds per SETUP.md) → 8. docs/hygiene → 9. GUI verification run + report of
verified vs needs-human (Accessibility/Screen-Recording grants, real Okta tenant round-trip).

**Open items needing a yes:** (a) license split GPL-3.0 app / Apache-2.0 server+protocol (or all
GPL-3.0); (b) backend choice above; (c) migration v3 (additive `groups.team` + `sync_meta`) —
PROMPT said "no schema changes", this is additive-only and uses the existing migration runner.
