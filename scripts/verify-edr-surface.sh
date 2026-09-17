#!/usr/bin/env bash
# Guard the macOS behaviors that previously produced SentinelOne's persistence alert.
set -euo pipefail

if rg -q 'MacosLauncher::LaunchAgent' src-tauri/src; then
  echo 'EDR surface check failed: Glyphio selects the plist-writing LaunchAgent backend.' >&2
  exit 1
fi

if ! rg -q 'MacosLauncher::AppleScript' src-tauri/src/lib.rs; then
  echo 'EDR surface check failed: Glyphio no longer selects the visible Login Items backend.' >&2
  exit 1
fi

if rg -q 'log_system_info\(' espanso/espanso/src; then
  echo 'EDR surface check failed: the bundled engine performs startup system-information discovery.' >&2
  exit 1
fi

echo 'EDR surface check passed: no Glyphio LaunchAgent backend or engine startup system discovery.'
