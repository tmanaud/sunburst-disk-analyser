#!/bin/sh
# Builds a universal (arm64 + x86_64) .app bundle and a .dmg in dist/.
#
# Signed ad hoc unless MACOS_SIGN_IDENTITY names a Developer ID Application
# certificate in the keychain. With NOTARY_KEY_P8 (an App Store Connect API
# key), NOTARY_KEY_ID and NOTARY_ISSUER as well, the app and the disk image
# are notarised and stapled.
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

IDENTITY="${MACOS_SIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ]; then
    # Ad-hoc signature: required on Apple Silicon, but not a Developer ID.
    codesign --force --deep --sign - "$APP"
else
    # The hardened runtime and a secure timestamp are required for notarisation.
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
fi
codesign --verify --deep --strict "$APP"

NOTARY_DIR=""
if [ -n "$IDENTITY" ] && [ -n "${NOTARY_KEY_P8:-}" ]; then
    : "${NOTARY_KEY_ID:?is needed with NOTARY_KEY_P8}" "${NOTARY_ISSUER:?is needed with NOTARY_KEY_P8}"
    NOTARY_DIR="$(mktemp -d)"
    trap 'rm -rf "$NOTARY_DIR"' EXIT
    printf '%s\n' "$NOTARY_KEY_P8" > "$NOTARY_DIR/key.p8"
fi
# notarise <file>: waits for Apple's verdict, then staples the ticket.
notarise() {
    xcrun notarytool submit "$1" --key "$NOTARY_DIR/key.p8" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" --wait
}
if [ -n "$NOTARY_DIR" ]; then
    # The app is stapled too, so it opens offline once copied out of the image.
    ditto -c -k --keepParent "$APP" "$NOTARY_DIR/app.zip"
    notarise "$NOTARY_DIR/app.zip"
    xcrun stapler staple "$APP"
fi

DMG="dist/Sunburst-Disk-Analyser-$VERSION-macos-universal.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
if [ -n "$IDENTITY" ]; then
    codesign --force --timestamp --sign "$IDENTITY" "$DMG"
fi
if [ -n "$NOTARY_DIR" ]; then
    notarise "$DMG"
    xcrun stapler staple "$DMG"
    spctl --assess --type execute -v "$APP"
fi
echo "Built $APP and $DMG"
