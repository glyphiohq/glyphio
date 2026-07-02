#!/usr/bin/env bash
#
# build-engine.sh — build the headless Glyphio engine (espanso fork) sidecar, place it where Tauri
# expects it, and sign it with the stable dev identity so the macOS Accessibility grant survives.
#
# Usage:  scripts/build-engine.sh            # debug build (for `npm run dev`)
#         scripts/build-engine.sh --release  # release build (for `npm run build`)
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -Vv | sed -n 's/host: //p')"

PROFILE_DIR="debug"
CARGO_PROFILE_FLAG=""
if [[ "${1:-}" == "--release" ]]; then
  PROFILE_DIR="release"
  CARGO_PROFILE_FLAG="--release"
fi

log() { printf '\033[36m[build-engine]\033[0m %s\n' "$*"; }

# 1. Build the headless espanso fork (no modulo/wxWidgets — no full Xcode needed).
log "Building engine ($PROFILE_DIR)…"
(cd "$REPO_ROOT/espanso" && \
  cargo build $CARGO_PROFILE_FLAG --no-default-features --features native-tls -p espanso)

# 2. Copy it in with Tauri's sidecar naming, and next to the dev exe for raw `cargo run`.
SRC="$REPO_ROOT/espanso/target/$PROFILE_DIR/espanso"
DEST_BIN="$REPO_ROOT/src-tauri/binaries/glyphio-engine-$TRIPLE"
DEST_EXE="$REPO_ROOT/src-tauri/target/$PROFILE_DIR/glyphio-engine"

mkdir -p "$(dirname "$DEST_BIN")"
cp "$SRC" "$DEST_BIN"
log "Copied → $DEST_BIN"
if [[ -d "$(dirname "$DEST_EXE")" ]]; then
  cp "$SRC" "$DEST_EXE"
  log "Copied → $DEST_EXE"
fi

# 3. Sign with the stable dev identity (creates it on first run).
"$REPO_ROOT/scripts/dev-sign.sh"
