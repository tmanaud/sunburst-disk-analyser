#!/bin/sh
# Builds a tar.gz and an AppImage for the host architecture in dist/.
set -eu
cd "$(dirname "$0")/.."
BIN=sunburst-disk-analyser
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="$(uname -m)"
cargo build --release

OUT="dist/Sunburst-Disk-Analyser-$VERSION-linux-$ARCH"
rm -rf "$OUT" dist/AppDir
mkdir -p "$OUT"
cp "target/release/$BIN" "$OUT/"
cp packaging/linux/$BIN.desktop assets/icon.png README.md LICENSE "$OUT/"
tar -C dist -czf "$OUT.tar.gz" "$(basename "$OUT")"
rm -rf "$OUT"

APPDIR=dist/AppDir
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/512x512/apps"
cp "target/release/$BIN" "$APPDIR/usr/bin/"
cp packaging/linux/$BIN.desktop "$APPDIR/usr/share/applications/"
cp packaging/linux/$BIN.desktop "$APPDIR/"
cp assets/icon.png "$APPDIR/$BIN.png"
cp assets/icon.png "$APPDIR/usr/share/icons/hicolor/512x512/apps/$BIN.png"
ln -sf "$BIN.png" "$APPDIR/.DirIcon"
# Keyboard libraries are loaded at run time and are missing on some minimal
# desktops; ship them. Graphics and Wayland libraries stay the system's.
mkdir -p "$APPDIR/usr/lib"
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1; do
    found="$(ldconfig -p | awk -v l="$lib" '$1 == l { print $NF; exit }')"
    if [ -n "$found" ]; then
        cp -L "$found" "$APPDIR/usr/lib/"
    else
        echo "warning: $lib not found on the build machine, not bundled" >&2
    fi
done
cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}$HERE/usr/lib"
exec "$HERE/usr/bin/sunburst-disk-analyser" "$@"
RUN
chmod +x "$APPDIR/AppRun"

# A fixed appimagetool release, checked against its known SHA-256 before it
# runs, whether it was just downloaded or was already there.
TOOL_VERSION=1.9.1
case "$ARCH" in
    x86_64) TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
    aarch64) TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
    *) echo "error: no known appimagetool checksum for $ARCH" >&2; exit 1 ;;
esac
TOOL="dist/appimagetool-$TOOL_VERSION-$ARCH"
if [ ! -f "$TOOL" ]; then
    curl -fsSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/$TOOL_VERSION/appimagetool-$ARCH.AppImage"
fi
if ! echo "$TOOL_SHA256  $TOOL" | sha256sum -c - >/dev/null; then
    echo "error: $TOOL does not match its expected checksum, not running it" >&2
    rm -f "$TOOL"
    exit 1
fi
chmod +x "$TOOL"
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "dist/Sunburst-Disk-Analyser-$VERSION-linux-$ARCH.AppImage"
rm -rf "$APPDIR"
echo "Built dist/Sunburst-Disk-Analyser-$VERSION-linux-$ARCH.{tar.gz,AppImage}"
