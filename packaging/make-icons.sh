#!/bin/sh
# Regenerates assets/icon.{png,ico,icns} from the procedural icon (macOS only for .icns).
set -eu
cd "$(dirname "$0")/.."
cargo build --release
BIN=target/release/sunburst-disk-analyser
"$BIN" --render-icon assets/icon.png 1024
"$BIN" --render-icon assets/icon.ico 256
if command -v iconutil >/dev/null 2>&1; then
    SET="$(mktemp -d)/icon.iconset"
    mkdir -p "$SET"
    for s in 16 32 128 256 512; do
        "$BIN" --render-icon "$SET/icon_${s}x${s}.png" "$s"
        "$BIN" --render-icon "$SET/icon_${s}x${s}@2x.png" "$((s * 2))"
    done
    iconutil -c icns "$SET" -o assets/icon.icns
fi
