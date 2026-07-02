# Glyphio — Phase 3 plan: enterprise collaboration (RBAC, admin console, scrolling capture)

Decisions delegated by the owner ("whichever way you believe is best"). Building order: A → D.

## A. RBAC (server-enforced, per-team)

Roles, strictly ordered: **owner > admin > manager > writer > reader**.

| capability | reader | writer | manager | admin | owner |
|---|---|---|---|---|---|
| pull team snippets/groups | ✓ | ✓ | ✓ | ✓ | ✓ |
| push own creates/edits | | ✓ | ✓ | ✓ | ✓ |
| push edits/tombstones of others' records | | | ✓ | ✓ | ✓ |
| view roster | ✓ | ✓ | ✓ | ✓ | ✓ |
| set roles ≤ manager | | | | ✓ | ✓ |
| set roles ≤ admin, rename/delete team | | | | | ✓ |

- **Enforcement lives in the server** (client UI only reflects it): push handler checks the
  caller's role per record (`owner` field = original author attribution).
- **Storage:** `roles(team, sub, role)` server-side. Bootstrap: the first identity to touch a
  team becomes its **owner**; OIDC group-claim members default to **writer** (env
  `DEFAULT_ROLE`); static-token config may pin roles per token.
- **Protocol (additive v1):** `Me` gains `roles: { "<team>": "writer", ... }`; pushes rejected
  by role return 403 problem+json per record batch.
- **Single-user:** no backend → no roles → full local control (unchanged).

## B. Admin console (bundled into the reference server)

A small static SPA served by the axum server at `/admin`, talking to an admin JSON API
(`/admin/v1/...`) gated to admin/owner roles. No second deployment; self-hosters get it with
`docker compose up`. Scope v1: team list, roster with roles, role assignment, team
create/rename/delete, seen-member activity. Same bearer auth (OIDC token paste or static
admin token first; full web OIDC redirect flow later).

## C. Scrolling capture

1. **Browser (full page + specific scrollable element):** companion browser extension ported
   from Checkpoint (owner's IP — this is literally its origin story): scroll-and-stitch the
   page, or click-to-pick a scrollable element and stitch just it. Talks to the desktop app
   via native messaging (Glyphio registers a native-messaging host; extension pushes the
   stitched PNG into the normal capture→edit→history flow).
2. **Native apps (experimental, after 1):** synthetic scroll-wheel + SCK frame capture +
   overlap correlation stitch; ships behind a setting flagged experimental.

## D. Snippets portability + organization

- **Export:** per-group or everything — Glyphio JSON (lossless: snippets + groups + scopes,
  minus sync bookkeeping). **Import:** Glyphio JSON or espanso YAML match files (adoption
  path); collision policy: new IDs, skip exact-duplicate triggers with a report.
- **Multi-team:** already live (identity carries all teams; every team syncs).
- Nested groups (parent_id, additive migration) — after RBAC lands.

## Sequencing

1. **Now:** export/import + UI polish (client); RBAC protocol + server enforcement + admin
   API + console skeleton (server agent, parallel).
2. Next: client role-awareness (badges, disabled edits for readers), admin console polish.
3. Then: companion extension (Checkpoint port) for scrolling capture; native stitch last.
