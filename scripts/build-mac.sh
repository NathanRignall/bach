#!/usr/bin/env bash
# Builds Bach.app and a DMG, signed with your Developer ID and notarized by Apple.
# Run on a Mac, from the repo root, inside the devShell:   nix develop -c scripts/build-mac.sh
#
# Needs, in the environment:
#   APPLE_SIGNING_IDENTITY  e.g. "Developer ID Application: Your Name (TEAMID)"
#                           (list yours with: security find-identity -v -p codesigning)
#   and notarization credentials, either an App Store Connect API key:
#     APPLE_API_ISSUER, APPLE_API_KEY (the key id), APPLE_API_KEY_PATH (the .p8 file)
#   or an Apple ID with an app-specific password:
#     APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID
set -euo pipefail

[[ "$(uname)" == Darwin ]] || { echo "build-mac.sh: run this on a Mac." >&2; exit 1; }

# Apple's codesign, xcrun and friends. A Nix shell can put sigtool's `codesign` first, which only
# signs ad hoc.
export PATH="/usr/bin:/usr/sbin:$PATH"

: "${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY (see the top of this script)}"
if ! security find-identity -v -p codesigning | grep -qF "$APPLE_SIGNING_IDENTITY"; then
  echo "build-mac.sh: \"$APPLE_SIGNING_IDENTITY\" isn't a signing identity in your keychain:" >&2
  security find-identity -v -p codesigning >&2
  exit 1
fi

if [[ -n "${APPLE_API_KEY:-}" ]]; then
  : "${APPLE_API_ISSUER:?set APPLE_API_ISSUER}" "${APPLE_API_KEY_PATH:?set APPLE_API_KEY_PATH}"
  notary=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [[ -n "${APPLE_ID:-}" ]]; then
  : "${APPLE_PASSWORD:?set APPLE_PASSWORD (an app-specific password)}" "${APPLE_TEAM_ID:?set APPLE_TEAM_ID}"
  notary=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
else
  echo "build-mac.sh: no notarization credentials (see the top of this script)." >&2
  exit 1
fi
xcrun notarytool --version >/dev/null

pnpm install --frozen-lockfile
# Tauri signs with APPLE_SIGNING_IDENTITY, then notarizes and staples the app.
cargo tauri build --bundles app,dmg "$@"

bundle=target/release/bundle
app="$bundle/macos/Bach.app"
dmgs=("$bundle"/dmg/Bach_*.dmg)
dmg=${dmgs[-1]}

# The DMG is what gets downloaded, so it's notarized and stapled too.
xcrun notarytool submit "$dmg" "${notary[@]}" --wait
xcrun stapler staple "$dmg"

echo "Checking what Gatekeeper will see:"
codesign --verify --deep --strict --verbose=2 "$app"
spctl --assess --type execute --verbose "$app"
xcrun stapler validate "$app"
spctl --assess --type open --context context:primary-signature --verbose "$dmg"
echo "Built: $app"
echo "       $dmg"
