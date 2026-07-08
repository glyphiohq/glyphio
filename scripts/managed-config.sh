#!/usr/bin/env bash
# Install (or remove) a system-wide managed sync config so you can test Glyphio's locked mode.
# The managed path is root-owned by design, so this uses sudo.
#
#   scripts/managed-config.sh                         # install config.managed.example.toml
#   scripts/managed-config.sh path/to/your.toml       # install a specific file
#   scripts/managed-config.sh --remove                # back to a personal (unmanaged) install
#
# After installing/removing, reopen Glyphio's Settings → Team sync (or use tray → Reload).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(uname)" == "Darwin" ]]; then
  DEST="/Library/Application Support/Glyphio/managed.toml"
else
  DEST="/etc/glyphio/managed.toml"
fi

if [[ "${1:-}" == "--remove" ]]; then
  echo "[managed-config] removing $DEST"
  sudo rm -f "$DEST"
  echo "[managed-config] removed — Glyphio is now an unmanaged (personal) install."
  exit 0
fi

SRC="${1:-$REPO_ROOT/config.managed.example.toml}"
[[ -f "$SRC" ]] || { echo "[managed-config] source not found: $SRC" >&2; exit 1; }

echo "[managed-config] installing $SRC → $DEST"
sudo mkdir -p "$(dirname "$DEST")"
sudo cp "$SRC" "$DEST"
sudo chown root:wheel "$DEST"
sudo chmod 644 "$DEST"
echo "[managed-config] installed. Reopen Settings → Team sync (or tray → Reload) to see locked mode."
