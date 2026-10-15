#!/usr/bin/env bash
# Publishes a release from main: sets the version, refreshes the README
# pictures, runs the tests and, once you have checked the pictures, commits,
# tags and pushes. The tag starts the build and release workflow on GitHub.
#
#   scripts/release.sh 1.1.0
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "release: $*" >&2; exit 1; }

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: scripts/release.sh <version, for example 1.1.0>"
tag="v$version"

[[ "$(git rev-parse --abbrev-ref HEAD)" == main ]] || die "switch to main first"
[[ -z "$(git status --porcelain)" ]] || die "commit or stash your changes first"
git fetch --quiet origin main --tags
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || die "main is not the same as origin/main"
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "$tag exists already"

start="$(git rev-parse HEAD)"
current="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
echo "Version $current -> $version"
perl -0pi -e 's/^version = "[^"]*"/version = "'"$version"'"/m' Cargo.toml
cargo update --quiet --workspace --offline 2>/dev/null || cargo update --quiet --workspace

echo "Tests"
cargo test --quiet
cargo clippy --quiet --all-targets -- -D warnings

echo "README pictures"
scripts/screenshots.sh

echo
git status --short
echo
echo "Check the pictures before publishing, for example:"
echo "  open -a Safari docs/screenshots/demo.webp; open docs/screenshots/*.png"
read -r -p "Commit, tag $tag and push? [y/N] " answer
if [[ "$answer" != [yY] ]]; then
    echo "Nothing was committed. To undo: git checkout -- Cargo.toml Cargo.lock docs/screenshots"
    exit 1
fi

# Nothing else may have changed meanwhile, for example from another editor.
[[ "$(git rev-parse --abbrev-ref HEAD)" == main && "$(git rev-parse HEAD)" == "$start" ]] \
    || die "main moved while the release was prepared; nothing was committed"
[[ -z "$(git diff --cached --name-only)" ]] || die "files were staged meanwhile; nothing was committed"
unexpected="$(git status --porcelain --untracked-files=all | cut -c4- \
    | grep -vE '^(Cargo\.toml|Cargo\.lock|docs/screenshots/.+)$' || true)"
[[ -z "$unexpected" ]] || die "unexpected changes, nothing was committed:
$unexpected"

git add Cargo.toml Cargo.lock docs/screenshots
git commit --quiet -m "release: $tag" -- Cargo.toml Cargo.lock docs/screenshots
git tag -a "$tag" -m "$tag"
git push --atomic origin main "$tag"
echo "Pushed $tag: the release workflow builds and publishes it."
