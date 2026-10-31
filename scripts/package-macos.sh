#!/bin/sh
# Builds a universal, ad-hoc signed TunnelBar.app and packages it as a DMG and zip in dist/.
set -eu
cd "$(dirname "$0")/.."
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
npm run tauri build -- --target universal-apple-darwin
APP=src-tauri/target/universal-apple-darwin/release/bundle/macos/TunnelBar.app
codesign --force --deep -s - "$APP"
rm -rf dist && mkdir -p dist/stage
cp -R "$APP" dist/stage/
ln -s /Applications dist/stage/Applications
hdiutil create -quiet -volname TunnelBar -srcfolder dist/stage -ov -format UDZO "dist/TunnelBar_${VERSION}_universal.dmg"
ditto -c -k --keepParent "$APP" "dist/TunnelBar_${VERSION}_universal.app.zip"
rm -rf dist/stage
(cd dist && shasum -a 256 *.dmg *.zip > SHA256SUMS.txt && cat SHA256SUMS.txt)
