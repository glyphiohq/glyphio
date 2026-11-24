#!/usr/bin/env bash
# Build the glyphio-ocr sidecar (Vision-framework OCR helper) and place it where Tauri
# bundles sidecars. Signed by dev-sign/sign-bundle like the engine.
#
# Usage: scripts/build-ocr.sh               # this Mac's architecture
#        scripts/build-ocr.sh --universal   # Apple Silicon + Intel, for releases
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -Vv | sed -n 's/host: //p')"
ARCHES=("${TRIPLE%%-*}")
if [[ "${1:-}" == "--universal" ]]; then
  TRIPLE="universal-apple-darwin"
  ARCHES=(arm64 x86_64)
fi
OUT="$REPO_ROOT/src-tauri/binaries/glyphio-ocr-$TRIPLE"
MIN_OS="14.0" # bundle.macOS.minimumSystemVersion in tauri.conf.json (enforced by release.sh)

echo "[build-ocr] compiling Vision OCR helper → $OUT"
# -target pins the deployment target. Without it swiftc embeds the BUILD machine's macOS
# as the binary's minimum (minos), and the sidecar silently refuses to run for anyone on
# an older macOS — this shipped once as minos 26.0. Keep in sync with
# bundle.macOS.minimumSystemVersion in tauri.conf.json (enforced by scripts/release.sh).
SLICES=()
for arch in "${ARCHES[@]}"; do
  [[ "$arch" == "aarch64" ]] && arch="arm64"
  slice="$OUT.$arch"
  swiftc -O -target "$arch-apple-macos$MIN_OS" -o "$slice" "$REPO_ROOT/scripts/ocr.swift" \
    -framework Vision -framework CoreImage -framework Foundation
  SLICES+=("$slice")
done
lipo -create -output "$OUT" "${SLICES[@]}"
rm -f "${SLICES[@]}"
codesign --force --sign "Glyphio Dev" --identifier "glyphio-ocr" "$OUT" 2>/dev/null \
  || echo "[build-ocr] note: dev identity missing — run scripts/dev-sign.sh first"
echo "[build-ocr] done"
