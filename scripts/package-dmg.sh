#!/usr/bin/env bash
# Package the SIGNED Glyphio.app into a distributable DMG. Must run AFTER sign-bundle.sh
# (Tauri's own DMG is built before re-signing and would ship the ad-hoc signature).
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$REPO_ROOT/src-tauri/target/release/bundle/macos/Glyphio.app"
VERSION=$(defaults read "$APP/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || echo dev)
OUT="$REPO_ROOT/src-tauri/target/release/bundle/share"
DMG="$OUT/Glyphio_${VERSION}_aarch64.dmg"

# refuse to package an unsigned/ad-hoc app
codesign -d -r- "$APP" 2>&1 | grep -q "certificate leaf" \
  || { echo "[package] FATAL: app is not identity-signed — run sign-bundle.sh first" >&2; exit 1; }

mkdir -p "$OUT"; rm -f "$DMG"
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
hdiutil create -volname "Glyphio" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
echo "[package] $DMG ($(du -h "$DMG" | cut -f1 | tr -d " "))"
