# Glyphio

**Text expansion + screenshot capture in one local-first, self-hostable desktop app.**

Glyphio pairs a hardened text-expansion engine (a thin fork of
[espanso](https://espanso.org)) with a screenshot capture / annotate / **redact** workflow,
wrapped in a native [Tauri](https://tauri.app) shell. Everything is local by default; an
optional, **bring-your-own-backend** sync shares chosen snippet groups with your team.

- **Snippets** — trigger → replacement, with plain / Markdown / rich-HTML bodies (rich text
  pastes with formatting intact, inline images included), espanso variables, folders, and
  per-app scoping (`Slack`, `exec:<regex>`, `title:<regex>`). Beyond text: **form**
  snippets (the trigger opens a small input form; the filled template is pasted),
  **popup** snippets (the trigger opens a cheatsheet window — images welcome), and
  **command** snippets (run a shell command, paste its output — strictly local-only,
  never synced).
- **Capture** — global hotkeys for screen / frontmost-window / browser-page (web content
  only, no browser chrome — via Accessibility, no extension needed) / picker / region /
  scrolling capture, a crop (drag-handle) + redact (black-out & blur) + draw + text editor, a
  timestamp/context banner you can add or remove later (the original capture time is
  always kept), and a device-local, re-editable history with size-capped eviction.
- **Team sync (opt-in)** — sign in with **any OIDC IdP** (Okta, Entra, Auth0, Keycloak, …)
  or a static API token, against **any backend implementing the
  [documented protocol](docs/SYNC-PROTOCOL.md)**. Offline-first, last-write-wins.
  **Personal snippets and all screenshots never leave the device** — redaction exists to
  keep sensitive pixels off shared surfaces, and sync respects that by design.

Platforms: macOS (Apple Silicon) today; Windows is on the roadmap.

## Quick start (build from source)

Prereqs: Rust stable, Node 18+, macOS Command Line Tools (full Xcode not required).

```bash
npm install
npm run engine       # build + sign the espanso-fork engine sidecar
npm run dev          # run the app (or: npm run release → dist/Glyphio_<version>_<arch>.dmg)
```

First run prompts for two macOS permissions (Accessibility for expansion, Screen Recording
for capture) — the in-app banners walk you through both.

## Self-hosting sync

A freshly built Glyphio makes **zero network calls** until you configure sync.

1. Run a backend: the Apache-2.0 reference server in [`server/`](server/) self-hosts with
   `docker compose up` (SQLite) and deploys to AWS serverless via [`infra/`](infra/)
   (Terraform) — or implement your own from [`docs/SYNC-PROTOCOL.md`](docs/SYNC-PROTOCOL.md).
2. Connect the app: Settings → Team sync (OIDC SSO or a static token).

**[SETUP.md](SETUP.md)** has the complete operator guide — IdP registration, server config,
where every credential lives (spoiler: OS keychain and your server's env, never this repo).

## Security posture

See [docs/SECURITY.md](docs/SECURITY.md): threat model, what syncs vs. what never does,
secret storage, and how to report vulnerabilities. Highlights: OIDC Authorization Code +
PKCE with full ID-token validation; tokens only in the OS keychain; TLS enforced; server
validates identity/team membership on every request; no telemetry, no auto-update pings,
no third-party calls.

## Repository layout

```
espanso/          vendored espanso fork (GPL-3.0, thin — see NOTICES.md)
src-tauri/        Tauri app + crates: snippet-store, sync-client, sync-proto
ui/               webview frontend (snippet manager, capture editor, history)
server/           reference sync backend (Apache-2.0; axum, SQLite/DynamoDB)
infra/            Terraform for the AWS reference deployment (Apache-2.0)
docs/             PHASE1.md, PHASE2-PLAN.md, SYNC-PROTOCOL.md, SECURITY.md
SETUP.md          operator / self-hosting guide
```

## License

**GPL-3.0-or-later** for the application (it contains the espanso fork), with Apache-2.0
carve-outs for the sync protocol types, reference server, and infra — so you can build
compatible servers/clients without copyleft obligations. Details in [NOTICES.md](NOTICES.md).

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).
