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

TOOL=dist/appimagetool
if [ ! -x "$TOOL" ]; then
    curl -fsSL -o "$TOOL" "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
    chmod +x "$TOOL"
fi
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "dist/Sunburst-Disk-Analyser-$VERSION-linux-$ARCH.AppImage"
rm -rf "$APPDIR"
echo "Built dist/Sunburst-Disk-Analyser-$VERSION-linux-$ARCH.{tar.gz,AppImage}"
