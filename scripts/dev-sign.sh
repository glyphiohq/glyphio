#!/usr/bin/env bash
#
# dev-sign.sh — sign the Glyphio engine sidecar (the expansion engine binary) with a STABLE self-signed identity so that the
# macOS Accessibility grant survives rebuilds during development.
#
# Why this exists
# ---------------
# macOS TCC (the Accessibility permission database) keys a grant to the binary's code identity.
# An UNSIGNED or ad-hoc-signed binary's identity (cdhash) changes on every `cargo build`, so the
# freshly-built engine is treated as a different, untrusted program even though System Settings
# still shows the old entry toggled on. Result: expansions silently stop working after a rebuild.
#
# Signing with a persistent self-signed certificate makes codesign emit a "designated requirement"
# based on the certificate identity (stable) rather than the cdhash (per-build). Grant Accessibility
# to the signed engine ONCE and it keeps working across rebuilds, as long as you keep signing with
# the same certificate.
#
# The certificate lives in a dedicated keychain (glyphio-dev.keychain-db) with a known password so
# the whole flow is non-interactive — it never touches your login keychain and never prompts.
#
# Usage:  scripts/dev-sign.sh            # ensure identity exists, then sign the sidecar
#         scripts/dev-sign.sh --reset    # delete the dev keychain and recreate it
#
set -euo pipefail

IDENTITY="Glyphio Dev"
# Must match the identifier Tauri assigns the bundled sidecar (derived from its filename), so the
# TCC/Accessibility designated requirement is identical between dev-run and release-bundle engines
# and a single grant covers both.
BUNDLE_ID="glyphio-engine"
KEYCHAIN_NAME="glyphio-dev.keychain-db"
KEYCHAIN_PATH="$HOME/Library/Keychains/$KEYCHAIN_NAME"
KEYCHAIN_PASS="glyphio-dev"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -Vv | sed -n 's/host: //p')"

# Candidate engine binaries to sign (only those that exist are signed).
CANDIDATES=(
  "$REPO_ROOT/src-tauri/binaries/glyphio-engine-$TRIPLE"
  "$REPO_ROOT/src-tauri/target/debug/glyphio-engine"
  "$REPO_ROOT/src-tauri/target/release/glyphio-engine"
)

log() { printf '\033[36m[dev-sign]\033[0m %s\n' "$*"; }
err() { printf '\033[31m[dev-sign]\033[0m %s\n' "$*" >&2; }

if [[ "${1:-}" == "--reset" ]]; then
  log "Removing dev keychain $KEYCHAIN_NAME"
  security delete-keychain "$KEYCHAIN_PATH" 2>/dev/null || true
fi

ensure_in_search_list() {
  if ! security list-keychains -d user | grep -q "$KEYCHAIN_NAME"; then
    local existing
    existing="$(security list-keychains -d user | sed -e 's/^[[:space:]]*"//' -e 's/"$//')"
    # shellcheck disable=SC2086
    security list-keychains -d user -s "$KEYCHAIN_PATH" $existing >/dev/null
  fi
}

create_identity() {
  log "Creating self-signed code-signing identity \"$IDENTITY\"…"
  local tmp cfg key crt p12
  tmp="$(mktemp -d)"
  cfg="$tmp/req.cnf"; key="$tmp/key.pem"; crt="$tmp/cert.pem"; p12="$tmp/id.p12"
  trap 'rm -rf "$tmp"' RETURN

  # Portable OpenSSL config (works with macOS's LibreSSL, which lacks `-addext`).
  cat > "$cfg" <<EOF
[req]
distinguished_name = dn
x509_extensions = codesign_ext
prompt = no
[dn]
CN = $IDENTITY
[codesign_ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF

  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$key" -out "$crt" -config "$cfg" >/dev/null 2>&1
  # OpenSSL 3+ defaults to AES-256 / SHA-256 PKCS12 which macOS `security import` cannot verify;
  # `-legacy` restores the RC2/SHA-1 encoding it understands. LibreSSL has no `-legacy` and already
  # writes the compatible encoding, so only pass it when supported.
  local legacy=""
  if openssl pkcs12 -help 2>&1 | grep -q -- '-legacy'; then legacy="-legacy"; fi
  # shellcheck disable=SC2086
  openssl pkcs12 -export $legacy -inkey "$key" -in "$crt" -out "$p12" \
    -name "$IDENTITY" -passout pass:"$KEYCHAIN_PASS" >/dev/null 2>&1

  # Dedicated keychain with a known password → fully non-interactive.
  security create-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN_PATH"
  security set-keychain-settings "$KEYCHAIN_PATH"                 # no auto-lock timeout
  security unlock-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN_PATH"
  security import "$p12" -k "$KEYCHAIN_PATH" -P "$KEYCHAIN_PASS" -T /usr/bin/codesign >/dev/null
  # Let codesign use the private key without an interactive prompt.
  security set-key-partition-list -S apple-tool:,apple:,codesign: \
    -s -k "$KEYCHAIN_PASS" "$KEYCHAIN_PATH" >/dev/null 2>&1 || true
  # Add our keychain to the search list so codesign can find the identity by name.
  ensure_in_search_list
  log "Identity \"$IDENTITY\" created in $KEYCHAIN_NAME"
}

# Ensure the identity exists and is discoverable by codesign. NOTE: `find-identity -v` only lists
# *trusted* identities, and our self-signed cert is deliberately untrusted — codesign can still use
# it by name. So we key idempotency on the keychain file existing, not on `find-identity`.
if [[ -f "$KEYCHAIN_PATH" ]]; then
  log "Reusing existing dev keychain $KEYCHAIN_NAME"
  security unlock-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN_PATH" 2>/dev/null || true
  ensure_in_search_list
else
  create_identity
fi

# Sign every engine binary that exists.
signed_any=0
for bin in "${CANDIDATES[@]}"; do
  if [[ -f "$bin" ]]; then
    log "Signing $bin"
    codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" --options runtime "$bin"
    codesign --verify --verbose=2 "$bin" 2>&1 | sed 's/^/    /' || true
    signed_any=1
  fi
done

# Also sign the MAIN APP binary with the same stable identity (its own identifier).
# macOS TCC keys the Screen Recording grant to the app's code identity exactly like it keys
# Accessibility to the engine's — an unsigned dev binary changes identity every cargo build,
# so the grant silently stops matching and the "grant Screen Recording" banner never clears.
APP_CANDIDATES=(
  "$REPO_ROOT/src-tauri/target/debug/glyphio"
  "$REPO_ROOT/src-tauri/target/release/glyphio"
)
for bin in "${APP_CANDIDATES[@]}"; do
  if [[ -f "$bin" ]]; then
    log "Signing app $bin"
    codesign --force --sign "$IDENTITY" --identifier "io.glyphio.app" "$bin"
    signed_any=1
  fi
done

if [[ "$signed_any" -eq 0 ]]; then
  err "No engine binary found. Build it first:"
  err "  (cd espanso && cargo build --no-default-features --features native-tls -p espanso)"
  err "  cp espanso/target/debug/espanso src-tauri/binaries/glyphio-engine-$TRIPLE"
  exit 1
fi

log "Done. Grant Accessibility to 'glyphio-engine' ONCE in System Settings › Privacy & Security ›"
log "Accessibility; the grant now survives rebuilds as long as you keep running this script."
