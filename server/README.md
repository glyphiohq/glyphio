# Glyphio sync server (reference backend)

The reference implementation of Glyphio's [v1 sync protocol](../docs/SYNC-PROTOCOL.md)
(`GET /v1/me`, `GET|POST /v1/teams/{team}/changes`, `GET /v1/teams/{team}/members`), plus
per-team RBAC, org governance (settings, team lifecycle, audit log) under `/admin/v1/...`,
and a bundled admin dashboard (`GET /admin`).
Anyone can implement the same protocol —
the Glyphio app is not coupled to this server, and this server is not coupled to any IdP or
cloud. Licensed Apache-2.0 so third-party backends can freely build on it.

- **Auth**: generic OIDC (any IdP's JWKS) and/or static API tokens — env-configured, both may
  be enabled at once.
- **Storage**: SQLite (self-host default) or DynamoDB (AWS serverless reference, see
  [`../infra/`](../infra/)).
- **Security**: per-team authorization on every request, server-stamped record ownership,
  request validation + body limits, per-credential rate limiting, no token/body logging.
  TLS is terminated in front (reverse proxy / API Gateway).
- **RBAC**: per-team roles `reader < writer < manager < admin < owner`, enforced server-side.

## Run locally (development)

```bash
# from the repository root — a throwaway static token:
TOKEN=$(openssl rand -hex 32)
echo "token (paste into the Glyphio app): $TOKEN"
SHA=$(printf '%s' "$TOKEN" | shasum -a 256 | cut -d' ' -f1)

cd server
STORAGE=sqlite DB_PATH=./dev.db \
STATIC_TOKENS="[{\"tokenSha256\":\"$SHA\",\"sub\":\"me\",\"teams\":[\"myteam\"]}]" \
cargo run
# → listening on 0.0.0.0:8787; point the app at http://127.0.0.1:8787 (auth mode: token)
```

## Self-host (docker compose)

```bash
cd server
cp .env.example .env   # fill in auth (OIDC and/or STATIC_TOKENS) — see the comments
docker compose up -d   # data persists in the glyphio-data volume
```

Put TLS in front (Caddy/Traefik/nginx/your LB) — clients refuse plain http for non-loopback
hosts. Health check: `GET /healthz`.


## Roles (RBAC)

Per team, strictly ordered: `reader < writer < manager < admin < owner`.

- **reader** — pull snippets/groups, view the roster. Pushes are rejected (403).
- **writer** — plus create records and edit/delete **their own**. Pushing someone else's
  record returns that record as `superseded` with the authoritative server copy (the
  protocol's reconcile path — the client applies it; nothing is overwritten).
- **manager** — plus edit/tombstone **others'** records (original authorship is preserved in
  `owner`), and add members to their team at `reader`/`writer`.
- **admin** — plus assign/remove roles up to `manager` (and only on members currently at or
  below `manager`), via the admin API/dashboard.
- **owner** — plus assign anything, including a second `owner` (ownership transfer: both are
  owners until one demotes the other).

Resolution order: explicit role row → token-pinned `"role"` (static tokens) → org default
role (org settings, falling back to the `DEFAULT_ROLE` env, default `writer`) for anyone in
the team's claim/config. **Bootstrap** (only while the org's team-creation policy is
`bootstrap`, the default): the first identity to touch a team that has no owner — and that
carries the team in its own IdP claim/token config — becomes its owner automatically. In
`owners`/`admins` modes, ownership comes only from explicit team creation. Group records
carry no ownership, so any writer may modify them.

`/v1/me` reports the resolved role per team in `roles`; clients reflect it (enforcement stays
server-side).

## Admin API + dashboard

Same bearer auth as `/v1`; team routes require `manager`+ on the team (grant ceilings per the
matrix above):

- `GET /admin/v1/teams` — teams you can administer (archived teams are hidden).
- `POST /admin/v1/teams` `{"team":"name"}` — create a team per the org `team_creation`
  policy (`owners`: any existing owner; `admins`: admin+; `bootstrap`: any authenticated
  identity). The creator becomes owner. Re-POSTing an archived team's name as its owner
  revives it.
- `DELETE /admin/v1/teams/{team}` — **archive** (owner only): data is kept, listings hide
  the team, sync answers 403 with detail `team archived`.
- `GET /admin/v1/teams/{team}/roles` — explicit role rows.
- `PUT /admin/v1/teams/{team}/roles/{sub}` `{"role":"manager"}` — assign (matrix above).
- `DELETE /admin/v1/teams/{team}/roles/{sub}` — remove the row (falls back to default).
- `GET /admin/v1/org` (any role-holder) / `PUT /admin/v1/org` (an owner of at least one team
  — single-org server: team owners ARE the org owners) — org settings: `defaultRole`,
  `teamCreation`, `exportTeamGroups` (`open`|`managers`|`disabled`, delivered to apps via
  `/v1/me` → `policy`), `auditRetentionDays`.
- `GET /admin/v1/audit?team=&limit=` — audit log; owners see everything, admins see their
  teams' entries. Entries: role changes, team lifecycle, org-settings changes, first-seen
  members, push batch **counts** — never snippet content or tokens. Retention is purged
  best-effort on write per `auditRetentionDays`.

`GET /admin` serves a self-contained dashboard (single file, no build step, no external
resources): Overview (teams/members/recent activity) · Teams (create/archive per policy,
roster with role dropdowns, add-member-by-sub) · Members (people across your teams) ·
Org settings (owners) · Audit (filterable). Views the caller can't use are hidden; the
token stays in tab memory.

## Joining and leaving (self-service membership)

An identity belongs to as many teams as it has access to — `/v1/me` unions the IdP claim with
explicit role rows — and members manage their own edges of that:

- `POST /v1/invites/redeem` `{"code":"…"}` — redeem an invite **as the calling identity**, using
  the credential they already have. The team is added on top of the ones they hold, so a second
  invite never displaces the first; a managed client locked to one backend can still join more of
  that backend's teams. The invite is consumed on success, unknown/expired/revoked codes all
  return the same error, and an existing higher role is never lowered.
- `DELETE /v1/teams/{team}/membership` — leave. Refused for a team's last owner (transfer
  ownership first). The team is also dropped from any invite token of theirs, so a stored
  credential can't keep a removed membership alive. Membership granted by an IdP claim can't be
  dropped here; the response says so (`stillGrantedByIdentityProvider`).

Both are audited (`invite.redeem`, `team.leave`).

## Invite tokens (day-to-day membership)

Managers+ mint invite tokens from the dashboard (Teams → Invite member) or via
`POST /admin/v1/teams/{team}/invites {"sub":"…","email":"…?","role":"…?","expiresDays":n?}`.
The server generates 32 random bytes, stores **only the SHA-256**, and returns the plaintext
**once** — hand it to the member, who pastes it into Glyphio's sync settings. Pinned roles are
capped by the inviter's ceiling (manager→writer, admin→manager, owner→admin; nobody mints
owner tokens). Revoke with `DELETE /admin/v1/teams/{team}/access/{sub}` (dashboard: "revoke
access"): the member's tokens lose that team (revoked outright when it was their only team)
and their role row is removed. Both are audited (`invite.create`, `access.revoke`) with no
token material.

Bearer resolution order: static env tokens (bootstrap) → stored invite tokens (expired/revoked
rejected) → OIDC.

## Restricted groups

Managers+ can mark a synced group **restricted** (dashboard: Teams → Groups, or
`PUT /admin/v1/teams/{team}/groups/{id}/restricted`). A restricted group and every snippet in
it are only serialized to identities holding a grant (`read` or `write`, managed at
`…/groups/{id}/acl/{sub}`) — managers+ always see them. `write` is required to push into the
group; a push without it gets a deliberately generic 403 (`forbidden`) so the group's
existence isn't confirmed. The change cursor advances globally — filtering happens at
serialization, so no per-user cursor state exists. Outgoing group records carry
`restricted: true` (server-set; client-supplied flags are stripped). Roster visibility is
unaffected — membership ≠ content visibility.

## Static tokens (bootstrap / back-compat)

```bash
openssl rand -hex 32                      # the token — goes in the app, never in config
printf '%s' '<token>' | shasum -a 256     # the digest — goes in STATIC_TOKENS
```

`STATIC_TOKENS='[{"tokenSha256":"<digest>","sub":"alice","email":"alice@example.com","teams":["myteam"]}]'`
(or `STATIC_TOKENS_FILE=/data/tokens.json`). Teams listed here are what the token may sync;
the optional `email` shows up in the team-members list, and an optional `"role"` pins the
token's role on all its teams (an explicit role row still overrides). Env tokens are meant
for **bootstrap** (the very first owner) and back-compat — day-to-day membership should use
invite tokens, which need no server restarts or env edits.

## OIDC

Set `OIDC_ISSUER` (the same issuer the app signs in against), `OIDC_AUDIENCE` (the app's
client ID) and `TEAMS_CLAIM` (default `groups`) — the server validates each bearer JWT's
signature (issuer JWKS, cached ~1h with rotation handling), `iss`, `aud`, `exp`, and reads
team membership from the claim. Works with Okta, Auth0, Entra ID, Keycloak, Google, etc.

## Environment reference

See [`.env.example`](.env.example) — every variable is documented there.

## AWS reference deployment

`Dockerfile` target `lambda` bundles the AWS Lambda Web Adapter; [`../infra/`](../infra/)
provisions ECR + Lambda + API Gateway (throttled) + DynamoDB with least-privilege IAM.

## Tests

```bash
cargo test        # storage LWW/pagination, auth, authz, end-to-end router test
```
