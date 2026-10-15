#!/usr/bin/env bash
# Renders the README pictures from the built-in demo data: the screenshots,
# the animation (docs/screenshots/demo.webp) and its still for visitors who
# ask for reduced motion. The window is drawn as on macOS, so run it on a Mac.
# Needs ffmpeg built with libwebp (Homebrew's is).
set -euo pipefail
cd "$(dirname "$0")/.."

command -v ffmpeg >/dev/null || { echo "ffmpeg is needed: brew install ffmpeg" >&2; exit 1; }
cargo build --release --quiet
bin=target/release/sunburst-disk-analyser
out=docs/screenshots
width=1600
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

common=(--lang en --demo-media assets/demo --size 1180x780)

# shot <scene> <file> <light|dark>
shot() {
    "$bin" --screenshot "$1" "$tmp/$2" --theme "$3" "${common[@]}"
    sips -Z "$width" "$tmp/$2" --out "$out/$2" >/dev/null
    echo "  $out/$2"
}

echo "Screenshots:"
shot overview overview.png light
shot columns columns.png light
shot biggest biggest-files.png light
shot review review.png light
shot overview dark.png dark
shot idle start.png light

echo "Animation:"
mkdir "$tmp/frames"
"$bin" --record "$tmp/frames" --theme light "${common[@]}"
ffmpeg -loglevel error -y -framerate 30 -i "$tmp/frames/frame-%04d.png" \
    -vf "scale=$width:-1:flags=lanczos" -c:v libwebp_anim -q:v 82 -compression_level 6 \
    -loop 1 -an "$out/demo.webp"
last="$(ls "$tmp/frames" | tail -1)"
sips -Z "$width" "$tmp/frames/$last" --out "$out/demo-still.png" >/dev/null
echo "  $out/demo.webp ($(du -h "$out/demo.webp" | cut -f1 | tr -d ' '))"
echo "  $out/demo-still.png"
