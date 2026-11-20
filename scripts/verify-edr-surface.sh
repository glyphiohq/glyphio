#!/usr/bin/env bash
# Guard the macOS behaviors that previously produced SentinelOne's persistence alert.
set -euo pipefail

if rg -q 'MacosLauncher::LaunchAgent' src-tauri/src; then
  echo 'EDR surface check failed: Glyphio selects the plist-writing LaunchAgent backend.' >&2
  exit 1
fi

# The login item is the system's own (SMAppService). Nothing may drive System Events from the
# normal launch path; the one-time removal of the old scripted item in `autostart::migrate_legacy`
# is the single allowed use, and no other file may spawn osascript.
if rg -q 'MacosLauncher::AppleScript' src-tauri/src; then
  echo 'EDR surface check failed: Glyphio selects the scripted (AppleScript) login item backend.' >&2
  exit 1
fi

if grep -rl 'osascript' src-tauri/src | grep -qv '/autostart\.rs$'; then
  echo 'EDR surface check failed: osascript is spawned outside the one-time login-item migration.' >&2
  exit 1
fi

if ! rg -q 'SMAppService' src-tauri/src/autostart.rs; then
  echo 'EDR surface check failed: Glyphio no longer uses the native login item service.' >&2
  exit 1
fi

if rg -q 'log_system_info\(' espanso/espanso/src; then
  echo 'EDR surface check failed: the bundled engine performs startup system-information discovery.' >&2
  exit 1
fi

echo 'EDR surface check passed: no Glyphio LaunchAgent backend or engine startup system discovery.'
