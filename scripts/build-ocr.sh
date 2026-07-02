#!/usr/bin/env bash
# Build the glyphio-ocr sidecar (Vision-framework OCR helper) and place it where Tauri
# bundles sidecars. Signed by dev-sign/sign-bundle like the engine.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -Vv | sed -n 's/host: //p')"
OUT="$REPO_ROOT/src-tauri/binaries/glyphio-ocr-$TRIPLE"

echo "[build-ocr] compiling Vision OCR helper → $OUT"
swiftc -O -o "$OUT" "$REPO_ROOT/scripts/ocr.swift" \
  -framework Vision -framework CoreImage -framework Foundation
codesign --force --sign "Glyphio Dev" --identifier "glyphio-ocr" "$OUT" 2>/dev/null \
  || echo "[build-ocr] note: dev identity missing — run scripts/dev-sign.sh first"
echo "[build-ocr] done"
