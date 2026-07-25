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

# ---- 3. build: signing identity, engine + OCR sidecars, app -----------------------
bash scripts/dev-sign.sh
bash scripts/build-engine.sh --release
npx tauri build

# ---- 4. sign with the stable identity (Tauri falls back to ad-hoc; see sign-bundle.sh)
bash scripts/sign-bundle.sh

# ---- 5. package the ONLY distributable: dist/Glyphio_<version>_<arch>.dmg ---------
APP="$BUNDLE/macos/Glyphio.app"
ARCH=$(uname -m)   # aarch64 naming kept for continuity with previous artifacts
[[ "$ARCH" == "arm64" ]] && ARCH="aarch64"
DMG="dist/Glyphio_${VERSION}_${ARCH}.dmg"

# The bundled app must carry the version we intend to ship.
BUILT_V=$(defaults read "$REPO_ROOT/$APP/Contents/Info.plist" CFBundleShortVersionString)
[[ "$BUILT_V" == "$VERSION" ]] || fail "built app reports $BUILT_V, expected $VERSION"

codesign -d -r- "$APP" 2>&1 | grep -q "certificate leaf" \
  || fail "app is not identity-signed — refusing to package"

mkdir -p dist
rm -f "$DMG"
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Glyphio $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

# ---- 6. verify + summarise --------------------------------------------------------
hdiutil verify "$DMG" >/dev/null
SHA=$(shasum -a 256 "$DMG" | cut -d' ' -f1)
log "OK — $DMG ($(du -h "$DMG" | cut -f1 | tr -d ' '))"
log "    app version : $BUILT_V"
log "    sha256      : $SHA"
