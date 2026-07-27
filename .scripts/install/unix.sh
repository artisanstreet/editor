#!/bin/sh
set -eu

repository=${ARTISAN_GITHUB_REPOSITORY:-sandersonstabo/artisan-editor}
version=${ARTISAN_VERSION:-latest}

case "$repository" in
	*[!A-Za-z0-9_.\/-]*|*/*/*|/*|*/|"")
		echo "ARTISAN_GITHUB_REPOSITORY must be an owner/repository pair." >&2
		exit 2
		;;
esac
case "$repository" in
	*/*) ;;
	*)
		echo "ARTISAN_GITHUB_REPOSITORY must be an owner/repository pair." >&2
		exit 2
		;;
esac
if [ "$version" != "latest" ] && ! printf '%s\n' "$version" |
	grep -Eq '^v?[0-9]+\.[0-9]+\.[0-9]+([+-][A-Za-z0-9.-]+)?$'; then
		echo "ARTISAN_VERSION must be 'latest' or a semantic version." >&2
		exit 2
fi

os=$(uname -s)
architecture=$(uname -m)
if [ "${ARTISAN_INSTALL_TEST_MODE:-0}" = "1" ]; then
	os=${ARTISAN_INSTALL_TEST_OS:-$os}
	architecture=${ARTISAN_INSTALL_TEST_ARCH:-$architecture}
fi

case "$architecture" in
	x86_64|amd64) architecture=x64 ;;
	arm64|aarch64) architecture=arm64 ;;
	*)
		echo "Artisan does not provide a bootstrap for architecture '$architecture'." >&2
		exit 2
		;;
esac

case "$os" in
	Darwin)
		target=macos-$architecture
		;;
	Linux)
		libc=
		if [ "${ARTISAN_INSTALL_TEST_MODE:-0}" = "1" ] && [ -n "${ARTISAN_INSTALL_TEST_LIBC:-}" ]; then
			libc=$ARTISAN_INSTALL_TEST_LIBC
		elif command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
			libc=musl
		else
			libc=gnu
		fi
		case "$libc" in
			gnu|musl) target=linux-$architecture-$libc ;;
			*)
				echo "Artisan does not recognize Linux libc '$libc'." >&2
				exit 2
				;;
		esac
		;;
	*)
		echo "Artisan does not provide a bootstrap for operating system '$os'." >&2
		exit 2
		;;
esac

asset=artisan-bootstrap-$target
if [ "$version" = "latest" ]; then
	release_path=latest/download
else
	release_path=download/$version
fi
release_base=https://github.com/$repository/releases/$release_path
asset_uri=$release_base/$asset
checksum_uri=$asset_uri.sha256
manifest_uri=$release_base/release-manifest.json
signature_uri=$release_base/release-manifest.sig

if [ "${ARTISAN_INSTALL_TEST_MODE:-0}" = "1" ] && [ "${ARTISAN_INSTALL_TEST_RESOLVE_ONLY:-0}" = "1" ]; then
	printf '%s\n' "target=$target" "asset=$asset" "asset_uri=$asset_uri" "checksum_uri=$checksum_uri" "manifest_uri=$manifest_uri" "version=$version"
	exit 0
fi

temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/artisan-bootstrap.XXXXXXXX")
bootstrap_path=$temporary_root/$asset
checksum_path=$bootstrap_path.sha256
cleanup() {
	rm -rf -- "$temporary_root"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

download() {
	uri=$1
	output=$2
	if command -v curl >/dev/null 2>&1; then
		curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
			--max-redirs 5 --output "$output" "$uri"
	elif command -v wget >/dev/null 2>&1; then
		wget --quiet --https-only --max-redirect=5 --output-document="$output" "$uri"
	else
		echo "Artisan installation requires curl or wget." >&2
		exit 1
	fi
}

download "$asset_uri" "$bootstrap_path"
download "$checksum_uri" "$checksum_path"

checksum_line=$(sed -n '1p' "$checksum_path" | tr -d '\r')
expected_digest=$(printf '%s\n' "$checksum_line" | awk '{ print $1 }')
case "$expected_digest" in
	*[!A-Fa-f0-9]*|"")
		echo "The Artisan bootstrap checksum sidecar is malformed." >&2
		exit 1
		;;
esac
if [ "${#expected_digest}" -ne 64 ]; then
	echo "The Artisan bootstrap checksum sidecar is malformed." >&2
	exit 1
fi
expected_digest=$(printf '%s' "$expected_digest" | tr 'A-F' 'a-f')

if command -v sha256sum >/dev/null 2>&1; then
	actual_digest=$(sha256sum "$bootstrap_path" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
	actual_digest=$(shasum -a 256 "$bootstrap_path" | awk '{ print $1 }')
else
	echo "Artisan installation requires sha256sum or shasum." >&2
	exit 1
fi
if [ "$expected_digest" != "$actual_digest" ]; then
	echo "The Artisan bootstrap failed SHA-256 verification." >&2
	exit 1
fi

chmod 700 "$bootstrap_path"
"$bootstrap_path" --manifest-url "$manifest_uri" --signature-url "$signature_uri" "$@"
