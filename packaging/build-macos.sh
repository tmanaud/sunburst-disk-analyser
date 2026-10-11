#!/bin/sh
# Builds a universal (arm64 + x86_64) .app bundle and a .dmg in dist/.
set -eu
cd "$(dirname "$0")/.."
NAME="Sunburst Disk Analyser"
BIN=sunburst-disk-analyser
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
export MACOSX_DEPLOYMENT_TARGET=11.0

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

APP="dist/$NAME.app"
rm -rf dist/macos "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/$BIN" \
    "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN"
sed "s/__VERSION__/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp assets/icon.icns "$APP/Contents/Resources/icon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# Ad-hoc signature: required on Apple Silicon, but not a Developer ID.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

DMG="dist/Sunburst-Disk-Analyser-$VERSION-macos-universal.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
echo "Built $APP and $DMG"
