#!/usr/bin/env bash
#
# sign-bundle.sh — sign the built Glyphio.app with a STABLE code identity.
#
# Usage: sign-bundle.sh [app-path] [identity]
#   Default identity is the self-signed "Glyphio Dev"; release.sh passes a real
#   "Developer ID Application: …" when the keychain has one.
#
# Tauri's own signing step silently falls back to AD-HOC because the self-signed dev
# certificate is deliberately untrusted (`security find-identity -v` hides it, and Tauri's
# identity lookup relies on that listing). An ad-hoc app changes code identity every build,
# which breaks every TCC grant — and on modern macOS the ENGINE's Accessibility check is
# attributed to the app (the "responsible process"), so the APP's identity is the one that
# must stay stable. This script re-signs inner sidecars first, then the bundle, and FAILS
# LOUDLY if the result isn't the cert-based designated requirement.
#
# Note for the day the Developer ID arrives: changing identity changes the app's designated
# requirement, so existing users re-grant Screen Recording and Accessibility once. Unavoidable,
# and much cheaper to do early than late.
set -euo pipefail

APP="${1:-src-tauri/target/release/bundle/macos/Glyphio.app}"
IDENTITY="${2:-Glyphio Dev}"

[[ -d "$APP" ]] || { echo "[sign-bundle] no bundle at $APP" >&2; exit 1; }

# Unlock the dedicated dev keychain (created by dev-sign.sh) so codesign never prompts.
KEYCHAIN="$HOME/Library/Keychains/glyphio-dev.keychain-db"
[[ -f "$KEYCHAIN" ]] && security unlock-keychain -p glyphio-dev "$KEYCHAIN" 2>/dev/null || true

# A real Developer ID needs the hardened runtime to be notarizable; the self-signed identity
# must NOT have it on the app, because the webview's JIT is blocked without an entitlement and
# an untrusted cert can't carry one usefully.
APP_FLAGS=()
if [[ "$IDENTITY" == "Developer ID Application:"* ]]; then
  APP_FLAGS=(--options runtime --entitlements "$(dirname "${BASH_SOURCE[0]}")/../src-tauri/entitlements.plist")
fi

# Inner-out: sidecars first, then the bundle.
for bin in "$APP"/Contents/MacOS/glyphio-engine*; do
  [[ -f "$bin" ]] && codesign --force --sign "$IDENTITY" --identifier "glyphio-engine" \
    --options runtime --timestamp "$bin"
done
for bin in "$APP"/Contents/MacOS/glyphio-ocr*; do
  [[ -f "$bin" ]] && codesign --force --sign "$IDENTITY" --identifier "glyphio-ocr" \
    --timestamp "$bin"
done
# `${APP_FLAGS[@]+...}` because macOS still ships bash 3.2, where expanding an empty array
# under `set -u` is an "unbound variable" error rather than nothing.
codesign --force --sign "$IDENTITY" --identifier "io.glyphio.app" \
  --timestamp ${APP_FLAGS[@]+"${APP_FLAGS[@]}"} "$APP"

# Assert: the app's designated requirement must be certificate-based (stable), never ad-hoc.
DR=$(codesign -d -r- "$APP" 2>&1 | tail -1)
if [[ "$DR" != *"certificate leaf"* ]]; then
  echo "[sign-bundle] FATAL: app signature is not certificate-based: $DR" >&2
  echo "[sign-bundle] TCC grants would break on every rebuild. Run scripts/dev-sign.sh first." >&2
  exit 1
fi
codesign --verify --deep "$APP"
echo "[sign-bundle] OK — $APP signed with stable identity ($DR)"
