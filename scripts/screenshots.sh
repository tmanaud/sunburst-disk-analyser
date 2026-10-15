#!/usr/bin/env bash
# Renders the README pictures from the built-in demo data: the screenshots,
# the animation (docs/screenshots/demo.webp) and its still for visitors who
# ask for reduced motion. The window is drawn as on macOS, so run it on a Mac.
# Needs ffmpeg built with libwebp (Homebrew's is).
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "screenshots: $*" >&2; exit 1; }

command -v ffmpeg >/dev/null || die "ffmpeg is needed: brew install ffmpeg"
# The binary Cargo just built, wherever its target folder is.
bin="$(cargo build --release --message-format=json-render-diagnostics \
    | sed -n 's/.*"executable":"\([^"]*\/sunburst-disk-analyser\)".*/\1/p' | tail -1)"
[[ -x "$bin" ]] || die "the build did not report the app binary"
out=docs/screenshots
width=1600
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

common=(--lang en --demo-media assets/demo --size 1180x780)

# Everything is made in $tmp/ready first: docs/ only changes once all succeeded.
mkdir "$tmp/ready"

# shot <scene> <file> <light|dark>
shot() {
    "$bin" --screenshot "$1" "$tmp/$2" --theme "$3" "${common[@]}"
    sips -Z "$width" "$tmp/$2" --out "$tmp/ready/$2" >/dev/null
    echo "  $2"
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
# frame-0001.png to frame-NNNN.png, none missing: ffmpeg would stop at a gap.
count="$(find "$tmp/frames" -name 'frame-*.png' | wc -l | tr -d ' ')"
last="$(ls "$tmp/frames" | tail -1)"
[[ "$count" -gt 0 && "$last" == "$(printf 'frame-%04d.png' "$count")" ]] || die "frames are missing from the recording"
ffmpeg -loglevel error -y -framerate 30 -i "$tmp/frames/frame-%04d.png" \
    -vf "scale=$width:-1:flags=lanczos" -c:v libwebp_anim -q:v 82 -compression_level 6 \
    -loop 1 -an "$tmp/ready/demo.webp"
sips -Z "$width" "$tmp/frames/$last" --out "$tmp/ready/demo-still.png" >/dev/null
echo "  demo.webp ($(du -h "$tmp/ready/demo.webp" | cut -f1 | tr -d ' '))"
echo "  demo-still.png"

cp "$tmp/ready/"* "$out/"
echo "Written to $out"
