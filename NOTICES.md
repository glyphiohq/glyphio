# Notices & Attribution

Glyphio is licensed **GPL-3.0-or-later** (see `LICENSE`), with two carve-outs below.

## Bundled / derived works

- **espanso** — `espanso/` is a fork of [espanso](https://github.com/espanso/espanso)
  (GPL-3.0, © Federico Terzi and contributors). Upstream provenance:
  `espanso/UPSTREAM_VERSION.txt`; the original `espanso/LICENSE` and all upstream copyright
  notices are preserved. The fork is deliberately thin — see `docs/PHASE1.md` for the exact
  deviations. Because Glyphio distributes this fork, the combined application is GPL-3.0.
- **Checkpoint** — the capture/annotate/redact editor (`ui/editor/`, `ui/history/`) is ported
  from Checkpoint, a Chrome extension owned by Glyphio's author, relicensed here under
  GPL-3.0-or-later. No third-party constraints.

## Differently-licensed components (carve-outs)

- **`src-tauri/crates/sync-proto`** — **Apache-2.0.** The wire-protocol types are permissively
  licensed so anyone can implement a compatible sync server or client without copyleft
  obligations.
- **`server/` and `infra/`** — **Apache-2.0.** The reference backend and its infrastructure
  code are separate programs that communicate with Glyphio over HTTP; they are permissively
  licensed for the same reason.

## Notable dependencies (not exhaustive; see each Cargo.toml / package.json)

- [Tauri](https://tauri.app) — MIT/Apache-2.0
- [openidconnect-rs](https://github.com/ramosbugs/openidconnect-rs) — MIT
- [keyring-rs](https://github.com/hwchen/keyring-rs) — MIT/Apache-2.0
- [rusqlite](https://github.com/rusqlite/rusqlite) — MIT
- [axum](https://github.com/tokio-rs/axum) — MIT
- Inter (SIL OFL 1.1) and Roboto Mono (Apache-2.0) fonts in `ui/shared/fonts/`
