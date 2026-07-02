# Glyphio security posture

## Threat model (short form)

Glyphio handles two sensitive things: **what you type** (a text expander must observe
keystrokes to detect triggers) and **what's on your screen** (captures may contain PII —
that's why the redaction tools exist). The design goal is that neither ever leaves the
device except content a user *deliberately* shares with a team, over a channel they control.

| asset | exposure | control |
|---|---|---|
| keystrokes | observed locally by the espanso-fork engine (macOS Accessibility) | never stored, never transmitted; engine is a local child process |
| screenshots + history | local disk only (`~/Library/Application Support/Glyphio/history`) | **no sync path exists** for history — not a policy, an absence of code |
| personal snippets | local SQLite | excluded from sync at the query level (`team IS NULL` records are never serialized) |
| team snippets | synced to *your* configured backend | TLS, bearer auth, server-side team authorization, LWW merge |
| credentials | OS keychain (macOS Keychain / Windows Credential Manager) | never in files, DB, logs, or config |

## Sync security

- **Auth:** OIDC Authorization Code + **PKCE (S256)** with `state` (CSRF) and `nonce`
  (replay) verification, loopback redirect per RFC 8252, ID-token signature/`iss`/`aud`/
  `exp` validation via the IdP's JWKS. Alternative: static API tokens (server stores SHA-256
  hashes only). Session refresh is silent; a revoked session drops to signed-out.
- **Transport:** HTTPS with standard certificate validation, enforced by config validation
  (loopback exempt for development).
- **Server:** validates the credential on every request, derives identity + team membership
  from it exclusively, enforces per-team authorization on read and write, applies input
  validation and size limits, rate-limits, and never logs tokens or record bodies.
  Least-privilege IAM in the reference AWS deployment.
- **Trust boundary:** the backend sees team snippet plaintext (v1 has no E2E encryption —
  documented future work). Deploy it on infrastructure you trust with that content.

## Client hardening

- Sync is **off by default**; a fresh build makes no network calls until configured.
- No telemetry, no auto-update pings, no third-party services.
- Config files carry no secrets; the app refuses `authMode`/URL combinations that would
  downgrade transport security.
- The espanso fork stays near-upstream so upstream security fixes rebase quickly.

## Reporting a vulnerability

Open a GitHub security advisory (preferred) or email the maintainer privately. Please do not
file public issues for exploitable problems. You can expect an acknowledgement within a few
days; fixes to the sync server or auth path take priority over everything else.
