#!/bin/sh
# Installs Artisan on Linux or macOS:
#
#   curl -fsSL https://github.com/sandersonstabo/artisan-editor/releases/latest/download/install.sh | sh
#   curl -fsSL …/install.sh | sh -s -- forge        (arguments go to `ae install`)
#
# Downloads release.json and this platform's archive, checks the archive's
# SHA-256 against release.json, and runs the `ae` inside with `install`.
# `ae install` reuses these files and verifies release.json's
# Ed25519 signature before installing anything. ARTISAN_RELEASES overrides
# the feed (a release.json URL or a directory holding a release).
set -eu

feed="${ARTISAN_RELEASES:-https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release.json}"
source="$feed"
case "$feed" in
    http://* | https://*) base="${feed%/*}" ;;
    *) base="$feed" feed="$feed/release.json" ;;
esac

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

fetch() {
    case "$1" in
        http://* | https://*) curl -fsSL --proto '=https,http' "$1" -o "$2" || fail "could not download $1" ;;
        *) cp "$1" "$2" || fail "could not read $1" ;;
    esac
}

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail "unsupported OS $(uname -s); on Windows run: irm ${base}/install.ps1 | iex" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported CPU $(uname -m)" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
fetch "$feed" "$tmp/release.json"
fetch "$feed.sig" "$tmp/release.json.sig"

# release.json lists one artifact per line.
line="$(grep "\"os\":\"$os\"" "$tmp/release.json" | grep "\"arch\":\"$arch\"" | head -n1 || true)"
[ -n "$line" ] || fail "this release has no build for $os $arch"
file="$(printf '%s\n' "$line" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')"
sha256="$(printf '%s\n' "$line" | sed -n 's/.*"sha256":"\([0-9a-fA-F]*\)".*/\1/p')"
case "$file" in
    "" | .* | */* | *[!A-Za-z0-9._+-]*) fail "unusable archive name '$file'" ;;
esac
[ -n "$sha256" ] || fail "release.json has no SHA-256 for $file"

echo "Downloading $file…"
fetch "$base/$file" "$tmp/$file"
if command -v sha256sum > /dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$file" | cut -d' ' -f1)"
else
    actual="$(shasum -a 256 "$tmp/$file" | cut -d' ' -f1)"
fi
[ "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" = "$(printf '%s' "$sha256" | tr 'A-F' 'a-f')" ] ||
    fail "$file is corrupt or was tampered with (SHA-256 $actual, expected $sha256)"

mkdir "$tmp/unpacked"
tar -xzf "$tmp/$file" -C "$tmp/unpacked" ae
# Piped into sh, stdin is this script; `ae install` may ask questions.
if [ -r /dev/tty ]; then
    ARTISAN_RELEASE_CACHE="$tmp" "$tmp/unpacked/ae" install --from "$source" "$@" < /dev/tty
else
    ARTISAN_RELEASE_CACHE="$tmp" "$tmp/unpacked/ae" install --from "$source" "$@"
fi
