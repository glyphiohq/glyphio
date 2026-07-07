# Glyphio — Operator & Self-Hosting Setup

How to connect a built Glyphio app to an identity provider and a sync backend.
**All credentials are supplied after build, at runtime — nothing in this repository or in a
Glyphio binary contains a tenant, endpoint, or secret.** Placeholders look `<LIKE_THIS>`.

Sync is **off by default**. A freshly built Glyphio makes zero network calls until you
complete one of the paths below.

---

## 1. Self-hoster path (any IdP, any backend)

Glyphio talks to any backend implementing [`docs/SYNC-PROTOCOL.md`](docs/SYNC-PROTOCOL.md)
and authenticates against any OIDC-compliant IdP — or none (static tokens).

### 1a. Stand up a backend

The reference server ships in [`server/`](server/) (Apache-2.0, so you may embed it in
anything). Quickest self-host:

```bash
cd server
cp .env.example .env         # fill it in (see server/README.md for every variable)
docker compose up -d         # SQLite storage in a named volume
```

Any HTTPS reverse proxy (Caddy, nginx, Traefik) in front of it completes the TLS
requirement. Or implement your own server — the protocol doc + `sync-proto` crate are the
whole contract.

### 1b. Choose an auth mode

**Option A — static API token (no IdP, simplest):**
1. Generate a token and its hash:
   ```bash
   TOKEN=$(openssl rand -hex 32); echo "token: $TOKEN"
   echo -n "$TOKEN" | shasum -a 256        # → tokenSha256 for the server config
   ```
2. Add `{ "tokenSha256": "<hash>", "sub": "alice", "teams": ["myteam"] }` to the server's
   `STATIC_TOKENS` (see `server/.env.example`).
3. In Glyphio: Settings → Team sync → auth mode **Static API token** → save → **Set API
   token…** → paste `$TOKEN`. It is stored in your OS keychain, nowhere else.

**Option B — OIDC single sign-on (any compliant IdP):**
1. Register a **native / public** app (no client secret) in your IdP with:
   - Grant type: **Authorization Code with PKCE (S256)**
   - Redirect URI: `http://127.0.0.1/callback` (loopback, any port — RFC 8252). If your IdP
     demands an exact port, pick one (e.g. `http://127.0.0.1:8912/callback`) and set
     `redirectPort = 8912` in Glyphio's sync config.
   - Scopes: `openid profile email offline_access` plus whatever scope carries group/team
     membership (commonly `groups`).
2. Make the IdP put team membership in an ID-token claim (array of strings). Default claim
   name is `groups`; the server's `TEAMS_CLAIM` env var changes it.
3. Configure the server with your issuer + audience (`OIDC_ISSUER`, `OIDC_AUDIENCE` =
   the client ID) so it can validate tokens via your IdP's JWKS.
4. In Glyphio: Settings → Team sync → fill **Backend URL**, **issuer**, **client ID**,
   scopes → save → **Sign in with SSO**.

### 1b-bis. Admin dashboard sign-in (`/admin`)

The server bundles the admin dashboard at `GET /admin`. It signs in two ways:

**OIDC in the browser (recommended):** the console runs Authorization Code + PKCE
client-side and uses the resulting ID token as its bearer.

1. Register a **second public client** in your IdP for the console (a "SPA" /
   browser client, no secret), with:
   - Grant type: **Authorization Code with PKCE (S256)**
   - Redirect URI: `https://<your-server>/admin` (exact match)
   - CORS / trusted origin: `https://<your-server>` (the browser calls the IdP's token
     endpoint directly)
2. Point the server at it: `ADMIN_OIDC_CLIENT_ID=<console client id>` (falls back to
   `OIDC_AUDIENCE` if you reuse one client) and optionally
   `ADMIN_OIDC_SCOPES="openid profile email groups"` (default `openid profile email`).
3. Open `/admin` → **Sign in with SSO**. The token lives in the tab's session storage
   only; the server still re-validates it on every request.

**Token paste (fallback / static-token deployments):** paste an admin/owner static token
or a raw OIDC ID token into the gate — kept in tab memory only, exactly as before.

### 1c. Where every value lives

| value | where |
|---|---|
| backend URL, issuer, client ID, scopes | `~/Library/Application Support/Glyphio/sync.toml` (or the in-app form; see `config.example.toml`) |
| OIDC refresh token | OS keychain (`io.glyphio.sync` / `oidc-refresh-token`) — written by the app |
| static API token | OS keychain (`io.glyphio.sync` / `static-token`) — written by the app |
| server-side secrets (token hashes, issuer config) | server environment / `.env` — never in this repo |

---

## 2. Enterprise reference path (example: Okta + AWS) (Okta Preview + sandbox AWS)

An enterprise deployment is **just configuration** of the generic machinery above.

### 2a. Register the Okta app (Okta Preview / sandbox org)

In Okta Admin → Applications → Create App Integration:
- Sign-in method: **OIDC**, Application type: **Native Application**
- Grant types: Authorization Code (+ Refresh Token), with **PKCE** (default for native)
- Sign-in redirect URI: `http://127.0.0.1/callback` (Okta allows any loopback port for
  native apps; if your org policy pins ports, use a fixed one + `redirectPort`)
- Assignments: the team's Okta group(s)
- Add a **groups claim** to the ID token (Security → API → your authorization server →
  Claims: name `groups`, include groups matching your team-name convention), so team
  membership reaches the app and the backend.

Record: **`<OKTA_ISSUER>`** (e.g. `https://<org>.oktapreview.com` or the custom
authorization server URL) and **`<OKTA_CLIENT_ID>`**.

### 2b. Deploy the reference backend to sandbox AWS

See [`infra/README.md`](infra/README.md) for the full steps. In short:

```bash
cd infra
cp terraform.tfvars.example terraform.tfvars   # fill in region, name prefix,
                                               # oidc_issuer=<OKTA_ISSUER>,
                                               # oidc_audience=<OKTA_CLIENT_ID>
# build + push the server image, then:
terraform init && terraform apply
```

Record the output **`<API_ENDPOINT>`** — that's the app's Backend URL. Everything
(account, region, issuer, audience, throttle rates) is a Terraform variable; nothing
the reference enterprise deployment-specific is committed.

### 2c. Point the app at it (after build)

In Glyphio → Settings → Team sync:

| field | value |
|---|---|
| Enable sync | on |
| Backend URL | `<API_ENDPOINT>` |
| Auth mode | OIDC single sign-on |
| OIDC issuer | `<OKTA_ISSUER>` |
| Client ID | `<OKTA_CLIENT_ID>` |
| Scopes | `profile email offline_access groups` |

Save → **Sign in with SSO** → share a snippet group with a team (⇅ next to the group).

### 2d. Still needed for production (not sandbox)

- Production Okta app registration (same shape as 2a, prod org + change control).
- Code signing + notarization of the app and the `glyphio-engine` sidecar.
- the reference enterprise deployment OSS/legal sign-off for public release of the GPL fork (see `docs/PHASE2-PLAN.md` §0).

---

## 3. Secrets checklist (audit before any release)

- [ ] `git status` / repo grep shows no issuer URLs, client IDs, account IDs, or tokens —
      only `<PLACEHOLDERS>` in `*.example*` files.
- [ ] `sync.toml`, `server/.env`, `infra/terraform.tfvars`, `*.tfstate` are gitignored
      (they are — see `.gitignore`).
- [ ] Tokens exist **only** in the OS keychain (app side) and as **SHA-256 hashes** or env
      config (server side).
- [ ] Nothing token-like appears in logs: the app never logs bearer values; the server
      never logs `Authorization` headers or record bodies.
- [ ] TLS everywhere except explicit `127.0.0.1` development.

---

## 4. Installing on managed (MDM/EDR) machines

On corporate Macs where your account is a standard user and EDR (e.g. SentinelOne) polices
`/Applications`, install per-user instead — no sudo, no quarantine changes, nothing for the
EDR to flag:

```bash
mkdir -p ~/Applications
cp -R src-tauri/target/release/bundle/macos/Glyphio.app ~/Applications/
open ~/Applications/Glyphio.app
```

`~/Applications` is a first-class macOS location (Spotlight/Launchpad index it) and all
TCC permissions (Accessibility, Screen Recording) work identically. Reserve `/Applications`
for MDM-distributed, properly signed builds.

---

## 5. Managed (enterprise) client configuration

To lock the sync connection so end users cannot change or redirect it (anti-exfiltration),
deploy a root-owned managed config via MDM:

- macOS: `/Library/Application Support/Glyphio/managed.toml`
- Linux: `/etc/glyphio/managed.toml`

Same format as `config.example.toml`. When present, the app uses it exclusively, ignores the
user's `sync.toml` connection fields, disables the connection form ("Managed by your
organization"), and rejects programmatic changes. Users still sign in with their own
credentials and manage their own snippet sharing. An invalid managed file locks sync **off**
rather than falling back to user config. Org-level policies (default role, team creation,
export policy, audit retention) live in the admin dashboard (`/admin` on your sync server).
