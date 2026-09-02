# Contributing to Glyphio

Thanks for considering it. Ground rules are short:

## Build & test

```bash
npm install
npm run engine                      # espanso-fork sidecar (headless build + dev signing)
npm run dev                         # run the app
cd src-tauri && cargo test          # snippet-store, sync-proto, sync-client tests
cd server && cargo test             # reference backend tests
node --check ui/settings/settings.js
```

## Where things go

- **Don't touch `espanso/` internals** unless the change genuinely belongs in the engine.
  The fork is deliberately thin so upstream security fixes rebase cleanly; app-level
  behavior belongs in `src-tauri/` or `ui/`. Any fork deviation must be documented in
  `docs/ARCHITECTURE.md`.
- **Wire-protocol changes** (`src-tauri/crates/sync-proto`, `docs/SYNC-PROTOCOL.md`) are
  breaking for every third-party server — additive within `/v1/`, path bump otherwise.
- **Secrets discipline**: no tenant IDs, endpoints, or tokens anywhere in the tree — only
  `<PLACEHOLDERS>` in `*.example*` files. Run the checklist in `SETUP.md` §3 before a PR.
- **No telemetry / phone-home** of any kind. Network calls happen only to a user-configured
  IdP and backend.

## Licensing of contributions

- App code (`src-tauri/`, `ui/`, `espanso/`): GPL-3.0-or-later.
- `sync-proto`, `server/`, `infra/`: Apache-2.0.

By submitting a PR you agree your contribution is licensed under the file's containing
component license (see `NOTICES.md`).
