#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
transport=$root/.scripts/install/unix.sh

sh -n "$transport"

resolve() {
	ARTISAN_INSTALL_TEST_MODE=1 \
	ARTISAN_INSTALL_TEST_RESOLVE_ONLY=1 \
	ARTISAN_INSTALL_TEST_OS=$1 \
	ARTISAN_INSTALL_TEST_ARCH=$2 \
	ARTISAN_INSTALL_TEST_LIBC=${3:-} \
	ARTISAN_VERSION=${4:-latest} \
	sh "$transport"
}

macos=$(resolve Darwin arm64)
printf '%s\n' "$macos" | grep -q '^target=macos-arm64$'
printf '%s\n' "$macos" | grep -q '^asset=artisan-bootstrap-macos-arm64$'

linux_gnu=$(resolve Linux x86_64 gnu v1.2.3)
printf '%s\n' "$linux_gnu" | grep -q '^target=linux-x64-gnu$'
printf '%s\n' "$linux_gnu" | grep -q \
	'^asset_uri=https://github.com/sandersonstabo/artisan-editor/releases/download/v1.2.3/artisan-bootstrap-linux-x64-gnu$'
printf '%s\n' "$linux_gnu" | grep -q \
	'^manifest_uri=https://github.com/sandersonstabo/artisan-editor/releases/download/v1.2.3/release-manifest.json$'

linux_musl=$(resolve Linux aarch64 musl)
printf '%s\n' "$linux_musl" | grep -q '^target=linux-arm64-musl$'

grep -q "mktemp -d" "$transport"
grep -q "trap cleanup EXIT" "$transport"
grep -q "trap 'exit 130' HUP INT TERM" "$transport"
grep -q "sha256sum" "$transport"
grep -q "shasum -a 256" "$transport"
grep -q -- "--proto '=https'" "$transport"
grep -q '"$bootstrap_path" --manifest-url "$manifest_uri" --signature-url "$signature_uri" "$@"' "$transport"

echo "POSIX install transport tests passed."
