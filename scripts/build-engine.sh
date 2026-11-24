#!/usr/bin/env bash
#
# build-engine.sh — build the headless Glyphio engine (espanso fork) sidecar, place it where Tauri
# expects it, and sign it with the stable dev identity so the macOS Accessibility grant survives.
#
# Usage:  scripts/build-engine.sh                        # debug build (for `npm run dev`)
#         scripts/build-engine.sh --release              # release build for this Mac
#         scripts/build-engine.sh --release --universal  # Apple Silicon + Intel, for releases
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -Vv | sed -n 's/host: //p')"

PROFILE_DIR="debug"
CARGO_PROFILE_FLAG=""
UNIVERSAL=0
for arg in "$@"; do
  case "$arg" in
    --release)   PROFILE_DIR="release"; CARGO_PROFILE_FLAG="--release" ;;
    --universal) UNIVERSAL=1 ;;
    *) echo "[build-engine] unknown argument: $arg" >&2; exit 1 ;;
  esac
done

log() { printf '\033[36m[build-engine]\033[0m %s\n' "$*"; }

build() { # [target triple]
  (cd "$REPO_ROOT/espanso" && \
    cargo build $CARGO_PROFILE_FLAG ${1:+--target "$1"} --no-default-features --features native-tls -p espanso)
}

# 1. Build the headless espanso fork (no modulo/wxWidgets — no full Xcode needed).
if [[ "$UNIVERSAL" == "1" ]]; then
  # A universal app needs every sidecar universal too: Tauri looks for
  # `glyphio-engine-universal-apple-darwin` and copies it into the bundle as is.
  log "Building engine ($PROFILE_DIR, arm64 + x86_64)…"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  build aarch64-apple-darwin
  build x86_64-apple-darwin
  SRC="$REPO_ROOT/espanso/target/universal/$PROFILE_DIR/espanso"
  mkdir -p "$(dirname "$SRC")"
  lipo -create -output "$SRC" \
    "$REPO_ROOT/espanso/target/aarch64-apple-darwin/$PROFILE_DIR/espanso" \
    "$REPO_ROOT/espanso/target/x86_64-apple-darwin/$PROFILE_DIR/espanso"
  TRIPLE="universal-apple-darwin"
else
  log "Building engine ($PROFILE_DIR)…"
  build
  SRC="$REPO_ROOT/espanso/target/$PROFILE_DIR/espanso"
fi

# 2. Copy it in with Tauri's sidecar naming, and next to the dev exe for raw `cargo run`.
DEST_BIN="$REPO_ROOT/src-tauri/binaries/glyphio-engine-$TRIPLE"
DEST_EXE="$REPO_ROOT/src-tauri/target/$PROFILE_DIR/glyphio-engine"

mkdir -p "$(dirname "$DEST_BIN")"
cp "$SRC" "$DEST_BIN"
chmod +x "$DEST_BIN" # cp preserves a pre-existing destination's mode — force executable
log "Copied → $DEST_BIN"
if [[ -d "$(dirname "$DEST_EXE")" ]]; then
  cp "$SRC" "$DEST_EXE"
  chmod +x "$DEST_EXE"
  log "Copied → $DEST_EXE"
fi

# 3. Sign with the stable dev identity (creates it on first run).
"$REPO_ROOT/scripts/dev-sign.sh"
