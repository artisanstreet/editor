#!/bin/sh
set -eu

feed="${ARTISAN_RELEASES:-https://github.com/artisanstreet/editor/releases/latest/download/release.json}"
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
    *) fail "unsupported OS $(uname -s); on Windows download Artisan Editor from ${base}" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported CPU $(uname -m)" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
fetch "$feed" "$tmp/release.json"

line="$(grep '"app":"ae"' "$tmp/release.json" | grep "\"os\":\"$os\"" | grep "\"arch\":\"$arch\"" | head -n1 || true)"
[ -n "$line" ] || fail "this release has no build for $os $arch"
file="$(printf '%s\n' "$line" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')"
sha256="$(printf '%s\n' "$line" | sed -n 's/.*"sha256":"\([0-9a-fA-F]*\)".*/\1/p')"
case "$file" in
    "" | .* | */* | *[!A-Za-z0-9._+-]*) fail "unusable file name '$file'" ;;
esac
[ -n "$sha256" ] || fail "release.json has no SHA-256 for $file"

echo "Downloading $file…"
fetch "$base/$file" "$tmp/ae"
if command -v sha256sum > /dev/null 2>&1; then
    actual="$(sha256sum "$tmp/ae" | cut -d' ' -f1)"
else
    actual="$(shasum -a 256 "$tmp/ae" | cut -d' ' -f1)"
fi
[ "$(printf '%s' "$actual" | tr 'A-F' 'a-f')" = "$(printf '%s' "$sha256" | tr 'A-F' 'a-f')" ] ||
    fail "$file is corrupt or was tampered with (SHA-256 $actual, expected $sha256)"

chmod +x "$tmp/ae"
if [ -r /dev/tty ] && [ -t 2 ]; then
    "$tmp/ae" install --feed "$source" "$@" < /dev/tty
else
    "$tmp/ae" install --feed "$source" "$@"
fi
