#!/usr/bin/env bash
#
# dev-roles.sh — mint one invite token per role on the local test server so you can
# experience Glyphio as a reader / writer / manager / admin.
#
# Usage:  scripts/dev-roles.sh [team] [owner-token] [server]
#   defaults:      e2e-team   e2e-test-token     http://127.0.0.1:8788
#
# To switch identity in the app: Settings → Sync → Sign out → Set API token… → paste a token
# below. (Each identity keeps its own server-side role; the app just presents the credential.)
# Watch role behavior:
#   reader  — pulls team snippets, edits stay local (push rejected), export may be blocked
#   writer  — edits own snippets; edits to others' snippets bounce back (superseded)
#   manager — edits anything in the team; sees restricted groups; can invite via dashboard
#   admin   — manager + role management in the dashboard (up to manager)
set -euo pipefail

TEAM="${1:-e2e-team}"
OWNER_TOKEN="${2:-e2e-test-token}"
SERVER="${3:-http://127.0.0.1:8788}"

echo "Minting role-test identities for team '$TEAM' on $SERVER"
echo

for ROLE in reader writer manager admin; do
  RESP=$(curl -sf -X POST \
    -H "Authorization: Bearer $OWNER_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"sub\":\"test-$ROLE\",\"email\":\"$ROLE@test.local\",\"role\":\"$ROLE\"}" \
    "$SERVER/admin/v1/teams/$TEAM/invites") || { echo "  $ROLE: FAILED (is the server up and the owner token valid?)"; continue; }
  TOKEN=$(echo "$RESP" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
  printf "  %-8s %s\n" "$ROLE" "$TOKEN"
done

echo
echo "Paste any token via Settings → Sync → Set API token… (sign out first)."
echo "Dashboard as that identity: open $SERVER/admin and paste the same token."
