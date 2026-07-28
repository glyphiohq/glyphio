#!/usr/bin/env bash
#
# update-cask.sh — point the Homebrew cask at a release that exists.
#
# Reads the built DMG in dist/, writes its version and sha256 into
# packaging/homebrew/glyphio.rb, and prints what to do with the result. Run after
# scripts/release.sh, before pushing the tap.
#
# Usage: scripts/update-cask.sh [version]   (defaults to package.json's version)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

log()  { printf '\033[36m[cask]\033[0m %s\n' "$*"; }
fail() { printf '\033[31m[cask] FATAL:\033[0m %s\n' "$*" >&2; exit 1; }

VERSION="${1:-$(node -p "require('./package.json').version")}"
ARCH=$(uname -m); [[ "$ARCH" == "arm64" ]] && ARCH="aarch64"
DMG="dist/Glyphio_${VERSION}_${ARCH}.dmg"
CASK="packaging/homebrew/glyphio.rb"

[[ -f "$DMG" ]] || fail "no $DMG — run scripts/release.sh first"

# The checksum has to come from the artifact that will actually be uploaded, so it is computed
# here rather than carried over from the release log.
SHA=$(shasum -a 256 "$DMG" | cut -d' ' -f1)

perl -pi -e "s/^  version \"[^\"]*\"/  version \"$VERSION\"/" "$CASK"
perl -pi -e "s/^  sha256 \"[^\"]*\"/  sha256 \"$SHA\"/" "$CASK"

log "$CASK now points at $VERSION"
log "    sha256: $SHA"
log ""
log "Next: copy to the tap and push it —"
log "    cp $CASK ../homebrew-tap/Casks/glyphio.rb"
log "  and make sure the v${VERSION} GitHub release carries $DMG."
