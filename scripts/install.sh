#!/bin/sh
set -eu

feed="${ARTISAN_RELEASES:-https://github.com/artisanstreet/editor/releases/latest/download/release.json}"
releases="https://github.com/artisanstreet/editor/releases/"
signer='artisan-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEAKSzXP/foIgie5gkF2r/s3uAf7Jus8WbSK5W+/Xk2L'
public_key='MCowBQYDK2VwAyEAQApLNc/9+giCJ7mCQXav+ze4B/sm6zxZtIrlb79eTYs='

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

source="$feed"
case "$feed" in
    https://*) base="${feed%/*}" ;;
    http://*) fail "refusing the unencrypted release feed $feed" ;;
    *) base="$feed" feed="$feed/release.json" ;;
esac

fetch() {
    case "$1" in
        https://*) curl -fsSL --proto '=https' "$1" -o "$2" || fail "could not download $1" ;;
        *) cp "$1" "$2" || fail "could not read $1" ;;
    esac
}

verify() {
    if command -v ssh-keygen > /dev/null 2>&1; then
        fetch "$base/release.json.sshsig" "$tmp/release.json.sshsig"
        printf '%s\n' "$signer" > "$tmp/signers"
        ssh-keygen -Y verify -f "$tmp/signers" -I artisan-release -n artisan-release -s "$tmp/release.json.sshsig" < "$tmp/release.json" > /dev/null 2>&1
    elif command -v openssl > /dev/null 2>&1; then
        fetch "$base/release.json.sig" "$tmp/release.json.sig"
        printf -- '-----BEGIN PUBLIC KEY-----\n%s\n-----END PUBLIC KEY-----\n' "$public_key" > "$tmp/release.pem"
        openssl base64 -d -A -in "$tmp/release.json.sig" -out "$tmp/release.json.sig.raw" > /dev/null 2>&1 &&
            openssl pkeyutl -verify -pubin -inkey "$tmp/release.pem" -rawin -in "$tmp/release.json" -sigfile "$tmp/release.json.sig.raw" > /dev/null 2>&1
    else
        fail "checking the release signature needs ssh-keygen (OpenSSH) or OpenSSL 3; install one and retry"
    fi
}

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail "unsupported OS $(uname -s); on Windows download Artisan Editor from https://github.com/artisanstreet/editor/releases" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "unsupported CPU $(uname -m)" ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM
fetch "$feed" "$tmp/release.json"

version="$(sed -n '1s/^{"version":"\([0-9A-Za-z.+-]*\)".*/\1/p' "$tmp/release.json")"
[ -n "$version" ] || fail "release.json names no usable version"
case "$feed" in
    "$releases"*) base="${releases}download/v${version%%+*}" ;;
esac
verify || fail "release.json is not signed by Artisan's release key"

line="$(grep '"app":"ae"' "$tmp/release.json" | grep "\"os\":\"$os\"" | grep "\"arch\":\"$arch\"" | head -n1 || true)"
[ -n "$line" ] || fail "this release has no build for $os $arch"
file="$(printf '%s\n' "$line" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')"
sha256="$(printf '%s\n' "$line" | sed -n 's/.*"sha256":"\([0-9a-fA-F]*\)".*/\1/p')"
case "$file" in
    "" | .* | */* | *[!A-Za-z0-9._+-]*) fail "unusable file name '$file'" ;;
esac
[ -n "$sha256" ] || fail "release.json has no SHA-256 for $file"

echo "Downloading Artisan $version ($file)…"
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
