#!/bin/bash
# Builds a universal Tunnel Bar.app signed with the Developer ID certificate, notarizes and staples the
# app and DMG, and produces the updater tarball, its signature and latest.json. Everything lands in dist/.
#
# Environment (all optional):
#   NOTES_FILE      Release notes for latest.json (default: "Tunnel Bar X.Y.Z").
#   SIGN_IDENTITY   codesign identity (default: the Developer ID Application certificate).
#   NOTARY_PROFILE  notarytool keychain profile (default: spicewood-notary).
#   UPDATER_KEY     Updater private key (default: ~/.tauri/tunnelbar-updater.key). Its password is read from
#                   the keychain item "tunnelbar-updater-key-password".
#   DOWNLOAD_BASE   Where the tarball will be published (default: this version's GitHub release).
#   EXTRA_CONFIG    Extra Tauri config JSON merged into the build (test builds only; needs ALLOW_TEST_BUILD=1).
#   SKIP_NOTARIZE=1 Sign but don't notarize (local experiments only; never publish the result).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -n "${TUNNELBAR_UPDATE_ENDPOINT:-}${TUNNELBAR_TEST_PROFILE:-}${EXTRA_CONFIG:-}" && "${ALLOW_TEST_BUILD:-}" != 1 ]]; then
  echo "Refusing to build a release with TUNNELBAR_UPDATE_ENDPOINT, TUNNELBAR_TEST_PROFILE or EXTRA_CONFIG set." >&2
  exit 1
fi

SIGN_IDENTITY=${SIGN_IDENTITY:-"Developer ID Application: Scott Van Vliet (97379Y67S5)"}
NOTARY_PROFILE=${NOTARY_PROFILE:-spicewood-notary}
UPDATER_KEY=${UPDATER_KEY:-"$HOME/.tauri/tunnelbar-updater.key"}
VERSION=$(node -p "Object.assign(require('./src-tauri/tauri.conf.json'), JSON.parse(process.env.EXTRA_CONFIG || '{}')).version")
DOWNLOAD_BASE=${DOWNLOAD_BASE:-"https://github.com/svanvliet/tunnel-bar/releases/download/v$VERSION"}
NAME="Tunnel-Bar_${VERSION}_universal"
TARBALL="$NAME.app.tar.gz"

[[ -f "$UPDATER_KEY" ]] || { echo "Updater key not found at $UPDATER_KEY" >&2; exit 1; }
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$(security find-generic-password -s tunnelbar-updater-key-password -w) \
  || { echo "Keychain item tunnelbar-updater-key-password not found" >&2; exit 1; }

notarize() {
  if [[ "${SKIP_NOTARIZE:-}" == 1 ]]; then echo "Skipping notarization of $1"; return; fi
  local out
  out=$(xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 30m) || true
  echo "$out"
  grep -q "status: Accepted" <<<"$out" \
    || { echo "Notarization failed for $1 (see: xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE)" >&2; exit 1; }
}

rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
# Signing and notarizing happen below, so keep Tauri from signing or creating updater artifacts itself.
env -u APPLE_SIGNING_IDENTITY -u TAURI_SIGNING_PRIVATE_KEY_PASSWORD -u TAURI_CONFIG \
  npm run tauri build -- --target universal-apple-darwin --bundles app ${EXTRA_CONFIG:+--config "$EXTRA_CONFIG"}
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/Tunnel Bar.app"

rm -rf dist && mkdir -p dist/stage
cp -R "$APP" dist/stage/
APP="dist/stage/Tunnel Bar.app"

# Hardened runtime, secure timestamp. No extra entitlements: the webview and devtunnel child processes need none.
codesign --force --deep --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

ditto -c -k --keepParent "$APP" dist/notarize.zip
notarize dist/notarize.zip
rm dist/notarize.zip
[[ "${SKIP_NOTARIZE:-}" == 1 ]] || xcrun stapler staple "$APP"

ditto -c -k --keepParent "$APP" "dist/$NAME.app.zip"
COPYFILE_DISABLE=1 tar -czf "dist/$TARBALL" -C dist/stage "Tunnel Bar.app"
npx tauri signer sign -f "$UPDATER_KEY" --app-version "$VERSION" "dist/$TARBALL" >/dev/null

ln -s /Applications dist/stage/Applications
hdiutil create -quiet -volname "Tunnel Bar" -srcfolder dist/stage -ov -format UDZO "dist/$NAME.dmg"
rm -rf dist/stage
codesign --force --timestamp --sign "$SIGN_IDENTITY" "dist/$NAME.dmg"
notarize "dist/$NAME.dmg"
[[ "${SKIP_NOTARIZE:-}" == 1 ]] || xcrun stapler staple "dist/$NAME.dmg"

NOTES_FILE=${NOTES_FILE:-} VERSION="$VERSION" SIG="$(cat "dist/$TARBALL.sig")" URL="$DOWNLOAD_BASE/$TARBALL" node -e '
  const fs = require("fs");
  const e = process.env;
  const target = { signature: e.SIG, url: e.URL };
  const manifest = {
    version: e.VERSION,
    notes: e.NOTES_FILE ? fs.readFileSync(e.NOTES_FILE, "utf8").trim() : `Tunnel Bar ${e.VERSION}`,
    pub_date: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    // One universal binary serves both architectures.
    platforms: { "darwin-aarch64": target, "darwin-x86_64": target },
  };
  fs.writeFileSync("dist/latest.json", JSON.stringify(manifest, null, 2) + "\n");
'

if [[ "${SKIP_NOTARIZE:-}" != 1 ]]; then
  mkdir -p dist/verify && ditto -x -k "dist/$NAME.app.zip" dist/verify
  spctl -a -vv -t exec "dist/verify/Tunnel Bar.app"
  xcrun stapler validate "dist/verify/Tunnel Bar.app"
  rm -rf dist/verify
  spctl -a -vv -t open --context context:primary-signature "dist/$NAME.dmg"
  xcrun stapler validate "dist/$NAME.dmg"
fi

(cd dist && shasum -a 256 ./*.dmg ./*.zip ./*.tar.gz latest.json | sed 's| \./| |' > SHA256SUMS.txt && cat SHA256SUMS.txt)
