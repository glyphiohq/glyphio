# Glyphio — Phase 4 plan: org governance, managed clients, isolation & anti-exfiltration

## A. Org model (server-side, first-class)

A single-org server (one deployment = one org — multi-tenant is out of scope; enterprises
deploy their own). New `org_settings` storage + `/admin/v1/org` API (owner-only):

- `default_role`, `team_creation` (`owners` | `admins` | `bootstrap` legacy), `allowed_teams`
  pattern (optional), `export_policy` (see §E), `audit_retention_days`.
- **Team lifecycle**: explicit `POST /admin/v1/teams` (per `team_creation` policy) replaces
  reliance on bootstrap-by-touch; bootstrap remains only as an opt-in compatibility mode and
  only ever fires for identities already carrying the team in their IdP claim.

## B. Admin dashboard v2 (proper dashboard, served at /admin)

Views: **Overview** (teams, members, activity counters) · **Teams** (create/rename/archive,
per-team roster + roles) · **Members** (cross-team view of people the caller administers) ·
**Org settings** (owner-only: §A knobs) · **Audit log** (§D). Role-scoped UI: owners see org
settings; admins see their teams; managers see only their team rosters (grant reader/writer).
Same single-binary serving, no external assets; ink & brass.

**Capability matrix (refined per owner direction):**

| | manager | admin | owner |
|---|---|---|---|
| add member to their team (reader/writer) | ✓ | ✓ | ✓ |
| grant per-group access (§C) | ✓ | ✓ | ✓ |
| add/remove managers | | ✓ | ✓ |
| create/archive teams (per policy) | | ✓ | ✓ |
| org settings, export policy, audit config | | | ✓ |

## C. Group-level access (within a team)

Model: a team group may be marked **restricted**. Restricted groups carry per-identity grants
(`group_acl(team, group_id, sub, level)` with level `read` | `write`). Server-side pull
filtering: snippets/groups in a restricted group are only serialized to identities holding a
grant (managers+ of the team always hold implicit write). Push validation mirrors it.
Unrestricted team groups behave as today (whole team per team role).
⚠ This touches pull filtering + cursor semantics (per-identity change visibility) — the
cursor stays global per team; filtering happens at serialization time so no per-user cursor
state is needed. Ships after A/B/D land.

## D. Audit log (server)

Append-only `audit(ts, actor_sub, action, team, target, detail)` for: role changes, team
lifecycle, org-setting changes, member additions, push batches (counts, not content),
export-policy denials. Owner-visible in the dashboard; retention per org settings. No
snippet bodies in the log (the log must not become the leak).

## E. Managed clients & anti-exfiltration posture

**Threat model honestly stated** (docs/SECURITY.md): Glyphio is local-first; a user can always
read their own local SQLite. "Anti-exfiltration" therefore means: (1) no cross-team/tenant
leakage server-side, (2) no silent redirection of team content to rogue backends, (3) no
bulk-export of team-shared content beyond policy, (4) auditability. It does not mean DRM over
a user's own personal snippets.

1. **Managed app config**: if `/Library/Application Support/Glyphio/managed.toml` exists
   (deployable via MDM; root-owned), its sync settings (backend URL, issuer, client id, auth
   mode) are **locked**: the app uses them, hides/disables the connection form (shows
   "Managed by your organization"), and refuses user overrides. Unmanaged installs behave as
   today (self-hosters/single users unaffected).
2. **Isolation guarantee** (verify + test, largely already true): data access strictly
   requires holding a role in that team — there is no super-admin read; owners/admins of team
   A see nothing of team B. Roster visibility ≠ content visibility. Add explicit
   cross-team-isolation tests including admin API paths.
3. **Export policy** (org setting, enforced app-side for team groups + reflected from
   `/v1/me` policy field): `open` (default) | `managers` (only manager+ may export
   team-shared groups) | `disabled` (no export of team-shared groups). Personal snippets
   always exportable (they're the user's own).
4. **Egress**: the app talks only to the configured backend + IdP (already true; restated as
   an invariant with the managed lock making it tamper-resistant).

## F. App settings UI (third pass)

Split Settings into clear sections with a segmented layout: **General** (capture/editor/
banner/history) · **Snippets** (defaults, import/export) · **Sync** (status card; connection
form hidden when managed) · **Permissions** (banners live here instead of floating) ·
**About** (version, licenses). Keyboard-shortcut capture row polish; consistent field grid.

## Sequencing

1. Server: org settings + team lifecycle + audit + tightened bootstrap (+ tests) — agent.
2. Dashboard v2 (Overview/Teams/Members/Org/Audit) — agent, same pass.
3. App: managed.toml lock + Settings IA restructure + export-policy enforcement — main.
4. §C restricted groups (protocol-additive `aclLevel` on pull; grants API + dashboard UI).
5. Cross-team isolation test suite + SECURITY.md threat-model update.
