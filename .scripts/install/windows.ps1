[CmdletBinding()]
param(
	[Parameter(ValueFromRemainingArguments = $true)]
	[string[]] $BootstrapArguments
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repository = if ($env:ARTISAN_GITHUB_REPOSITORY) {
	$env:ARTISAN_GITHUB_REPOSITORY
} else {
	"sandersonstabo/artisan-editor"
}
$version = if ($env:ARTISAN_VERSION) { $env:ARTISAN_VERSION } else { "latest" }

if ($repository -notmatch "^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$") {
	throw "ARTISAN_GITHUB_REPOSITORY must be an owner/repository pair."
}
if ($version -ne "latest" -and $version -notmatch "^v?[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?$") {
	throw "ARTISAN_VERSION must be 'latest' or a semantic version."
}

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($env:ARTISAN_INSTALL_TEST_MODE -eq "1" -and $env:ARTISAN_INSTALL_TEST_ARCH) {
	$architecture = $env:ARTISAN_INSTALL_TEST_ARCH.ToLowerInvariant()
}
$target = switch ($architecture) {
	"x64" { "windows-x64" }
	"arm64" { "windows-arm64" }
	default { throw "Artisan does not provide a Windows bootstrap for architecture '$architecture'." }
}

$asset = "artisan-bootstrap-$target.exe"
$release_path = if ($version -eq "latest") { "latest/download" } else { "download/$version" }
$release_base = "https://github.com/$repository/releases/$release_path"
$asset_uri = "$release_base/$asset"
$checksum_uri = "$asset_uri.sha256"
$manifest_uri = "$release_base/release-manifest.json"
$signature_uri = "$release_base/release-manifest.sig"

if ($env:ARTISAN_INSTALL_TEST_MODE -eq "1" -and $env:ARTISAN_INSTALL_TEST_RESOLVE_ONLY -eq "1") {
	[pscustomobject]@{
		asset = $asset
		asset_uri = $asset_uri
		checksum_uri = $checksum_uri
		manifest_uri = $manifest_uri
		target = $target
		version = $version
	} | ConvertTo-Json -Compress
	exit 0
}

$temporary_root = Join-Path ([System.IO.Path]::GetTempPath()) ("artisan-bootstrap-" + [guid]::NewGuid().ToString("N"))
$bootstrap_path = Join-Path $temporary_root $asset
$checksum_path = "$bootstrap_path.sha256"

try {
	[System.IO.Directory]::CreateDirectory($temporary_root) | Out-Null
	Invoke-WebRequest -UseBasicParsing -MaximumRedirection 5 -Uri $asset_uri -OutFile $bootstrap_path
	Invoke-WebRequest -UseBasicParsing -MaximumRedirection 5 -Uri $checksum_uri -OutFile $checksum_path

	$checksum_text = [System.IO.File]::ReadAllText($checksum_path).Trim()
	if ($checksum_text -notmatch "^(?<digest>[A-Fa-f0-9]{64})(?:\s+\*?[^\s]+)?$") {
		throw "The Artisan bootstrap checksum sidecar is malformed."
	}
	$expected_digest = $Matches.digest.ToLowerInvariant()
	$actual_digest = (Get-FileHash -LiteralPath $bootstrap_path -Algorithm SHA256).Hash.ToLowerInvariant()
	if ($expected_digest -cne $actual_digest) {
		throw "The Artisan bootstrap failed SHA-256 verification."
	}

	& $bootstrap_path --manifest-url $manifest_uri --signature-url $signature_uri @BootstrapArguments
	if ($LASTEXITCODE -ne 0) {
		throw "The Artisan bootstrap exited with code $LASTEXITCODE."
	}
} finally {
	if ([System.IO.Directory]::Exists($temporary_root)) {
		[System.IO.Directory]::Delete($temporary_root, $true)
	}
}
