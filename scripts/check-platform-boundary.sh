#!/usr/bin/env bash
# The platform boundary (src-tauri/src/platform/mod.rs): OS conditionals and OS crates live
# only under src-tauri/src/platform. Everything else asks `platform::current()`, so a new
# operating system is a new directory there and nothing else. Run by CI; exits 1 on a breach.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='cfg!?\([^)]*\b(target_os|target_family|unix|windows)\b|std::os::(unix|windows|macos)|\b(core_graphics|core_foundation|screencapturekit|objc2[a-z_]*|libc|windows_sys|windows_core|block2)::|kind *= *"framework"|RunEvent::Reopen'

hits=$(grep -rnE "$pattern" src-tauri/src src-tauri/examples src-tauri/tests src-tauri/crates \
  --include='*.rs' | grep -v '^src-tauri/src/platform/' || true)

if [ -n "$hits" ]; then
  echo "OS-specific code outside src-tauri/src/platform:" >&2
  echo "$hits" >&2
  echo "::error::move it behind a trait or helper in src-tauri/src/platform" >&2
  exit 1
fi
echo "platform boundary holds"
