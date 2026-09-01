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
  scrolling capture (press `Esc` to stop early while keeping the frames it has), a crop
  (drag-handle) + redact (black-out & blur) + draw + text editor, a
  timestamp/context banner you can add or remove later (the original capture time is
  always kept), and a device-local, re-editable history with size-capped eviction.
- **Clipboard history** — everything you copy, text and images, pasted back into the app you
  were in. Pin what you want to keep; entry and size caps evict the rest. **Content a password
  manager marks concealed is never recorded**, apps can be excluded by name, and none of it
  syncs.
- **One palette for all of it** — `Alt+Space` (or the single menu-bar entry) opens a
  searchable window with three lists: **Clipboard** (where it opens), **Capture** modes, and
  **Snippets**. `Tab` or `⌘1`/`⌘2`/`⌘3` between them; `↩` does the obvious thing for whichever
  list you're in and `⌘↩` the useful alternative. In-app, **History** shows screenshots and
  clipboard entries on one timeline under **All / Text / Images**.
- **Team sync (opt-in)** — sign in with **any OIDC IdP** (Okta, Entra, Auth0, Keycloak, …)
  or a static API token, against **any backend implementing the
  [documented protocol](docs/SYNC-PROTOCOL.md)**. Offline-first, last-write-wins.
  **Personal snippets and all screenshots never leave the device** — redaction exists to
  keep sensitive pixels off shared surfaces, and sync respects that by design.

Platforms: macOS (Apple Silicon) today; Windows is on the roadmap.

## Install

Download `Glyphio_<version>_aarch64.dmg` from
[Releases](https://github.com/glyphiohq/glyphio/releases) and drag it to Applications.

Glyphio isn't notarized yet — a Developer ID costs $99/year and this is a donation-funded
project — so the first launch needs one trip through System Settings → Privacy & Security →
**Open Anyway**. You do that once. Full details, including checksum verification, are in
**[docs/INSTALL.md](docs/INSTALL.md)**.

A Homebrew cask is written and kept in step with each release (`packaging/homebrew/`), but the
tap isn't published yet, so there is no `brew install` line to copy today.

First run prompts for two macOS permissions (Accessibility for expansion, Screen Recording
for capture) — the in-app banners walk you through both.

## Build from source

Prereqs: Rust stable, Node 18+, and the **macOS 26 SDK** (Command Line Tools are enough;
full Xcode is not). The SDK floor comes from a transitive dependency — `screencapturekit`
requires `apple-metal`, which references Metal symbols introduced in macOS 26. The app
itself still *runs* on macOS 14 and later.

```bash
npm install
npm run engine       # build + sign the espanso-fork engine sidecar
npm run dev          # run the app (or: npm run release → dist/Glyphio_<version>_<arch>.dmg)
```

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
validates identity/team membership on every request; no telemetry and no third-party calls.

The one network call an unconfigured Glyphio makes is the update check: on launch it asks
GitHub whether a newer release exists, sending nothing but the request itself. Updates are
verified against Glyphio's own signing key before installation. Turn it off in
Settings → About if you'd rather check yourself.

## Repository layout

```
espanso/          vendored espanso fork (GPL-3.0, thin — see NOTICES.md)
src-tauri/        Tauri app + crates: snippet-store, sync-client, sync-proto
ui/               webview frontend (snippet manager, capture editor, history)
server/           reference sync backend (Apache-2.0; axum, SQLite/DynamoDB)
infra/            Terraform for the AWS reference deployment (Apache-2.0)
packaging/        Homebrew cask (source of truth for the tap)
docs/             INSTALL.md, SYNC-PROTOCOL.md, SECURITY.md, PHASE*.md
SETUP.md          operator / self-hosting guide
```

## License

**GPL-3.0-or-later** for the application (it contains the espanso fork), with Apache-2.0
carve-outs for the sync protocol types, reference server, and infra — so you can build
compatible servers/clients without copyleft obligations. Details in [NOTICES.md](NOTICES.md).

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).
