#!/usr/bin/env bash
#
# release.sh — the one command that produces a consistent, distributable Glyphio build.
#
# Usage:
#   scripts/release.sh                  # build with the current version
#   scripts/release.sh --bump patch     # bump the version first (patch | minor | major)
#   scripts/release.sh --version 1.2.0  # set an explicit version first
#
# Version has ONE source of truth: package.json. This script propagates it to
# src-tauri/tauri.conf.json and src-tauri/Cargo.toml, refuses to build if the three ever
# disagree, and leaves exactly ONE distributable artifact: dist/Glyphio_<version>_<arch>.dmg.
# Everything under src-tauri/target/release/bundle/ is intermediate and gets cleaned of
# stale DMGs so old builds can't be mistaken for current ones.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

log()  { printf '\033[36m[release]\033[0m %s\n' "$*"; }
fail() { printf '\033[31m[release] FATAL:\033[0m %s\n' "$*" >&2; exit 1; }

# ---- 0. arguments ---------------------------------------------------------------
BUMP="" NEW_VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --bump)    BUMP="${2:?--bump needs patch|minor|major}"; shift 2 ;;
    --version) NEW_VERSION="${2:?--version needs X.Y.Z}"; shift 2 ;;
    *) fail "unknown argument: $1 (use --bump patch|minor|major or --version X.Y.Z)" ;;
  esac
done

# ---- 1. version: bump if asked, then propagate package.json -> tauri.conf/Cargo ----
if [[ -n "$BUMP" && -n "$NEW_VERSION" ]]; then fail "--bump and --version are exclusive"; fi
if [[ -n "$BUMP" ]]; then
  npm version "$BUMP" --no-git-tag-version >/dev/null
elif [[ -n "$NEW_VERSION" ]]; then
  [[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "version must be X.Y.Z"
  npm version "$NEW_VERSION" --no-git-tag-version --allow-same-version >/dev/null
fi
VERSION=$(node -p "require('./package.json').version")

node -e '
  const fs = require("fs");
  const v = require("./package.json").version;
  const confPath = "src-tauri/tauri.conf.json";
  const conf = JSON.parse(fs.readFileSync(confPath, "utf8"));
  if (conf.version !== v) {
    conf.version = v;
    fs.writeFileSync(confPath, JSON.stringify(conf, null, 2) + "\n");
  }
'
# Cargo.toml: replace only the [package] version (first `version =` line).
perl -0pi -e "s/^version = \"[^\"]*\"/version = \"$VERSION\"/m" src-tauri/Cargo.toml
# Keep Cargo.lock in step so the working tree stays clean after the build.
(cd src-tauri && cargo update --workspace --offline --quiet 2>/dev/null || true)

# Consistency gate — the three files must agree before anything is built.
TAURI_V=$(node -p "require('./src-tauri/tauri.conf.json').version")
CARGO_V=$(perl -ne 'if (/^version = "([^"]+)"/) { print $1; exit }' src-tauri/Cargo.toml)
[[ "$VERSION" == "$TAURI_V" && "$VERSION" == "$CARGO_V" ]] \
  || fail "version drift: package.json=$VERSION tauri.conf.json=$TAURI_V Cargo.toml=$CARGO_V"
log "version $VERSION (package.json = tauri.conf.json = Cargo.toml)"

# ---- 2. clean stale artifacts so old builds can't be shipped by accident ----------
BUNDLE="src-tauri/target/release/bundle"
rm -f  "$BUNDLE"/macos/*.dmg "$BUNDLE"/macos/rw.*.dmg 2>/dev/null || true
rm -rf "$BUNDLE"/dmg "$BUNDLE"/share 2>/dev/null || true

# ---- 3. decide who signs this build ----------------------------------------------
# Two different signatures are involved and they answer different questions:
#
#   * Apple's  — will Gatekeeper open the app on someone else's Mac?
#   * minisign — is this update genuinely from us? (see src-tauri/src/updates.rs)
#
# The minisign key is ours and always available. Apple's requires a paid Developer ID, so this
# script uses one if the keychain has one and falls back to the self-signed dev identity if not.
# Nothing here needs editing the day a Developer ID appears — import it and rebuild.
DEV_ID=$(security find-identity -v -p codesigning 2>/dev/null \
         | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)
if [[ -n "$DEV_ID" ]]; then
  log "signing with $DEV_ID (will notarize)"
  NOTARIZE=1
else
  log "no Developer ID in the keychain — falling back to the self-signed \"Glyphio Dev\" identity"
  NOTARIZE=0
fi

# ---- 4. build: signing identity, engine + OCR sidecars, app -----------------------
bash scripts/dev-sign.sh
bash scripts/build-engine.sh --release
bash scripts/build-ocr.sh

# The updater artifact is signed with the project's minisign key, which lives outside the repo.
UPDATER_KEY="${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.glyphio/updater.key}"
if [[ -f "$UPDATER_KEY" ]]; then
  export TAURI_SIGNING_PRIVATE_KEY="$UPDATER_KEY"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
else
  fail "no updater signing key at $UPDATER_KEY — run: npx tauri signer generate -w \"$UPDATER_KEY\""
fi

npx tauri build

# ---- 5. sign with a stable identity (Tauri falls back to ad-hoc; see sign-bundle.sh)
if [[ "$NOTARIZE" == "1" ]]; then
  bash scripts/sign-bundle.sh "$BUNDLE/macos/Glyphio.app" "$DEV_ID"
else
  bash scripts/sign-bundle.sh
fi

# ---- 5. package the ONLY distributable: dist/Glyphio_<version>_<arch>.dmg ---------
APP="$BUNDLE/macos/Glyphio.app"
ARCH=$(uname -m)   # aarch64 naming kept for continuity with previous artifacts
[[ "$ARCH" == "arm64" ]] && ARCH="aarch64"
DMG="dist/Glyphio_${VERSION}_${ARCH}.dmg"

# The bundled app must carry the version we intend to ship.
BUILT_V=$(defaults read "$REPO_ROOT/$APP/Contents/Info.plist" CFBundleShortVersionString)
[[ "$BUILT_V" == "$VERSION" ]] || fail "built app reports $BUILT_V, expected $VERSION"

# Every bundled executable must run on the app's declared minimum macOS. A binary built
# without an explicit deployment target inherits the BUILD machine's OS as its minimum
# (glyphio-ocr once shipped requiring macOS 26) — catch that before it reaches users.
MIN_OS=$(node -p "require('./src-tauri/tauri.conf.json').bundle.macOS.minimumSystemVersion")
for bin in "$APP"/Contents/MacOS/*; do
  [[ -f "$bin" && -x "$bin" ]] || continue
  MINOS=$(otool -l "$bin" | awk '/LC_BUILD_VERSION|LC_VERSION_MIN_MACOSX/{f=1} f && /minos|version /{print $2; exit}')
  [[ -n "$MINOS" ]] || fail "could not read minimum OS of $(basename "$bin")"
  HIGHER=$(printf '%s\n%s\n' "$MIN_OS" "$MINOS" | sort -V | tail -1)
  [[ "$HIGHER" == "$MIN_OS" ]] \
    || fail "$(basename "$bin") requires macOS $MINOS but the app declares $MIN_OS — rebuild it with a pinned deployment target"
done
log "all bundled binaries run on macOS $MIN_OS+"

codesign -d -r- "$APP" 2>&1 | grep -q "certificate leaf" \
  || fail "app is not identity-signed — refusing to package"

mkdir -p dist
rm -f "$DMG"
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Glyphio $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

# ---- 6. notarize, if we have an Apple identity to do it with ----------------------
# Notarization is what stops Gatekeeper refusing a downloaded build. Stapling the ticket to
# the DMG means it also works on a Mac that is offline the first time it opens Glyphio.
if [[ "$NOTARIZE" == "1" ]]; then
  if [[ -z "${APPLE_ID:-}" || -z "${APPLE_PASSWORD:-}" || -z "${APPLE_TEAM_ID:-}" ]]; then
    fail "Developer ID found but APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID are not set (use an app-specific password)"
  fi
  log "submitting to Apple for notarization (this takes a few minutes)…"
  xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait || fail "notarization failed"
  xcrun stapler staple "$DMG" || fail "could not staple the notarization ticket"
  spctl -a -vv -t install "$DMG" 2>&1 | grep -q "accepted" \
    || fail "Gatekeeper still rejects the notarized DMG"
  log "notarized and stapled — Gatekeeper accepts this build"
else
  log "NOT notarized: users will need System Settings → Privacy & Security → Open Anyway"
  log "               once, on first launch (see docs/INSTALL.md)"
fi

# ---- 7. updater manifest ----------------------------------------------------------
# `latest.json` is what a running Glyphio fetches to learn a new version exists. It points at
# the .app.tar.gz Tauri just signed with the project's minisign key; the signature travels in
# the manifest, so a tampered download fails verification rather than installing.
UPDATER_TGZ=$(ls "$BUNDLE"/macos/*.app.tar.gz 2>/dev/null | head -1)
UPDATER_SIG=$(ls "$BUNDLE"/macos/*.app.tar.gz.sig 2>/dev/null | head -1)
if [[ -n "$UPDATER_TGZ" && -n "$UPDATER_SIG" ]]; then
  cp "$UPDATER_TGZ" "dist/Glyphio_${VERSION}_${ARCH}.app.tar.gz"
  TARGET_KEY="darwin-aarch64"
  [[ "$ARCH" == "x86_64" ]] && TARGET_KEY="darwin-x86_64"
  node -e '
    const fs = require("fs");
    const [version, target, sigPath, url] = process.argv.slice(1);
    const manifest = {
      version,
      notes: `Glyphio ${version}`,
      pub_date: new Date().toISOString(),
      platforms: { [target]: { signature: fs.readFileSync(sigPath, "utf8").trim(), url } },
    };
    fs.writeFileSync("dist/latest.json", JSON.stringify(manifest, null, 2) + "\n");
  ' "$VERSION" "$TARGET_KEY" "$UPDATER_SIG" \
    "https://github.com/glyphiohq/glyphio/releases/download/v${VERSION}/Glyphio_${VERSION}_${ARCH}.app.tar.gz"
  log "updater manifest: dist/latest.json ($TARGET_KEY)"
else
  fail "no updater artifact was produced — check bundle.createUpdaterArtifacts in tauri.conf.json"
fi

# ---- 8. verify + summarise --------------------------------------------------------
hdiutil verify "$DMG" >/dev/null
SHA=$(shasum -a 256 "$DMG" | cut -d' ' -f1)
log "OK — $DMG ($(du -h "$DMG" | cut -f1 | tr -d ' '))"
log "    app version : $BUILT_V"
log "    sha256      : $SHA"
log "    notarized   : $([[ "$NOTARIZE" == "1" ]] && echo yes || echo 'no (self-signed)')"
log ""
log "To publish: upload $DMG, dist/Glyphio_${VERSION}_${ARCH}.app.tar.gz and dist/latest.json"
log "            to the v${VERSION} GitHub release."
