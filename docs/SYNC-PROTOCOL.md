# Glyphio Sync Protocol — v1

The wire contract between a Glyphio client and any compatible sync backend. Anyone can
implement a server (or an alternative client) against this document; the executable form of
the record/request/response types lives in `src-tauri/crates/sync-proto` (Apache-2.0).

## Design summary

- **Transport:** HTTPS + JSON. Plain HTTP is only permitted to `127.0.0.1`/`localhost` for
  development.
- **Auth:** `Authorization: Bearer <credential>` on every request. The credential is either an
  **OIDC ID token** (any compliant IdP; the server validates signature/`iss`/`aud`/`exp`
  against the issuer's JWKS) or a **static API token** (server compares SHA-256 hashes).
  The server derives `sub`, `email`, and **team membership** from the validated credential —
  clients never assert their own authorization.
- **Ordering:** a **server-assigned monotonic cursor** (`since` / `nextCursor`). Client clocks
  never order the stream. The cursor is opaque to clients beyond "pass it back".
- **Conflicts:** last-write-wins per record on `(updatedAt, version)` — `updatedAt` is RFC3339
  UTC with milliseconds (lexicographically chronological), `version` breaks same-millisecond
  ties, exact ties keep the holder's copy. Both sides apply the same rule (`sync_proto::lww_wins`).
- **Deletes:** tombstones — `deletedAt` set, records never physically removed from the stream.
- **Scope:** only **team** records exist in this protocol. Personal snippets and capture
  history are not represented and must never be transmitted.
- **Versioning:** the URL path (`/v1/…`). Additive response fields are allowed within v1;
  breaking changes bump the path.

## Records

`SnippetRec` (camelCase JSON):

| field | type | notes |
|---|---|---|
| `id` | string (uuid) | stable across devices |
| `trigger` | string | ≤ 512 bytes |
| `replacement` | string | ≤ 1 MiB (sized for inline data-URI images in rich bodies) |
| `format` | string | `plain` \| `markdown` \| `html` |
| `kind` | string? | *(additive)* `text` (default when absent) \| `form` \| `popup`. `command` is **not a legal wire value** — see the executable-content rule below |
| `variables` | array? | espanso `vars`, ≤ 16 KiB serialized. `shell`/`script` variable types are **rejected on push** — see below |
| `groupId` | string? | folder reference |
| `appScope` | string? | per-app activation filter |
| `owner` | string | **server-set** from the authenticated `sub` on push |
| `team` | string | must equal the `{team}` path segment |
| `updatedAt` | string | RFC3339 UTC, millisecond precision |
| `version` | integer | per-record edit counter |
| `deletedAt` | string? | tombstone marker |

`GroupRec`: `id`, `name`, `sortOrder`, `team`, `updatedAt`, `version`, `deletedAt?`.

## Endpoints

### `GET /v1/me`
Identity attested by the server from the bearer credential.

```json
{
  "sub": "00u...",
  "email": "user@example.com",
  "teams": ["secops", "platform"],
  "roles": { "secops": "owner", "platform": "writer" }
}
```

Clients sync exactly the teams listed here.

`roles` *(additive)*: the caller's per-team role — `reader < writer < manager < admin <
owner`. **Enforcement is always server-side**; clients use this only to shape UI. Servers
without RBAC omit the field (clients assume `writer`). Semantics: readers pull but cannot
push; writers push their own records; managers may modify/tombstone others' records; admins
manage roles up to manager; owners manage everything including the team itself. Role
management happens out-of-band of this protocol (the reference server exposes an admin API +
console at `/admin`; the IdP or server config remains the identity source).

### `GET /v1/teams/{team}/changes?since=<cursor>&limit=<n>`
Everything after `since`, oldest first. `limit` defaults to 200, max 1000.

```json
{ "snippets": [...], "groups": [...], "nextCursor": 4132, "more": false }
```

`more: true` = the page was truncated; pull again immediately with `since = nextCursor`.

### `POST /v1/teams/{team}/changes`
Batch push of locally-changed records (≤ 500 records total per batch):

```json
{ "snippets": [...], "groups": [...] }
```

The server LWW-merges each record and answers per record:

```json
{
  "snippets": [
    { "id": "…", "status": "accepted" },
    { "id": "…", "status": "superseded", "serverRecord": { ... } }
  ],
  "groups": [...],
  "cursor": 4140
}
```

- `accepted` — the pushed record won and is now authoritative; the client acknowledges it.
- `superseded` — the server already holds a newer record (returned in `serverRecord`); the
  client applies it locally through the same LWW rule.

The server **overrides `owner`** with the authenticated `sub` and **rejects (422)** any record
whose `team` differs from the path.

### `GET /v1/teams/{team}/members` *(additive)*
The team roster as known to the server, sorted by `sub`:

```json
{ "members": [ { "sub": "00u...", "email": "a@example.com", "lastSeen": "2026-07-02T06:03:50.548Z" } ] }
```

Sources: in static-token mode, the configured token list; in OIDC mode, identities recorded as
they authenticate ("seen members"). **Membership is owned by the IdP or the server's token
config — there is deliberately no write API for it** (clients guide admins to the right place
instead). `lastSeen` is absent for configured-but-never-seen members. Optional endpoint:
servers without a roster concept may 404 it; clients must degrade gracefully.

## Errors

Non-2xx responses carry an RFC 7807 problem document:

```json
{ "title": "Forbidden", "status": 403, "detail": "not a member of team \"secops\"" }
```

| status | meaning | client behaviour |
|---|---|---|
| 401 | missing/invalid/expired credential | drop to signed-out; re-auth |
| 403 | not a member of `{team}` | skip the team; surface the error |
| 413 | body over 1 MiB | split the batch |
| 422 | validation failure (limits, team mismatch) | surface; do not retry unchanged |
| 429 | rate limited (`Retry-After` honored) | back off |
| 5xx | server fault | exponential backoff, retry |

## Client obligations

1. Pull before push; pull again after push to advance the cursor past your own writes.
2. Apply pulled records through LWW — never blind-overwrite local state.
3. Persist per-team cursors and per-record acknowledged versions; the dirty set (version >
   acknowledged) is the offline queue. Never drop local edits on failure.
4. Serialize **only** records whose `team` is in the `/v1/me` team list.
5. Enforce TLS (loopback exempt), never log credentials.
6. **Executable content never syncs.** Exclude from push any snippet with `kind: command`
   or `shell`/`script` variables; **quarantine on pull** any record carrying them (strip
   the executable variables and apply it disabled) — this must hold even against a
   non-compliant server.

## Server obligations

1. Validate the credential on **every** request; derive identity/teams from it exclusively.
2. Enforce team authorization on both read and write; never leak cross-team data.
3. Assign a monotonic per-team (or global) sequence to every accepted write.
4. Merge with the same LWW rule; return `superseded` records rather than silently dropping.
5. Enforce the validation limits (`sync_proto::limits`); rate-limit; never log tokens or
   record bodies.
6. **Reject executable content** (422 problem+json): any pushed snippet with
   `kind: command` or a `variables` entry of type `shell`/`script`
   (`sync_proto::has_exec_vars`). A synced shell command would be remote code execution
   on every member's machine; rejection tells the pusher rather than silently sanitizing.

## Trust model (v1)

The server sees team snippet content in plaintext — deploy it somewhere you trust with that
content (your own infra). End-to-end encryption of record bodies is explicitly future work
(would move LWW metadata outside the encrypted envelope). Personal snippets and screenshots
never reach any server, so the exposure is limited to content deliberately shared with a team.

## Restricted groups *(additive)*

A team group may be **restricted** (managed via the reference server's admin API/dashboard;
other servers may implement equivalently). Semantics:

- `GroupRec.restricted: true` on outgoing records (server-set; client-supplied values are
  ignored and stripped).
- Pull filtering happens **server-side at serialization**: identities without a grant (and
  below manager) receive neither the group record nor any snippet in it; the cursor still
  advances normally.
- Grants are per-identity `read` or `write`. Pushing into a restricted group without `write`
  (or manager+) yields a **batch-level generic 403** ("forbidden", no detail) so the group's
  existence is not confirmed to non-granted identities.
- Clients need no special handling: unseen records simply never arrive.
