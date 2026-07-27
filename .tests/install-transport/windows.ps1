$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$script_path = Resolve-Path (Join-Path $PSScriptRoot "..\..\.scripts\install\windows.ps1")

function Resolve-Transport([string] $Architecture, [string] $Version = "latest") {
	$previous_mode = $env:ARTISAN_INSTALL_TEST_MODE
	$previous_resolve = $env:ARTISAN_INSTALL_TEST_RESOLVE_ONLY
	$previous_architecture = $env:ARTISAN_INSTALL_TEST_ARCH
	$previous_version = $env:ARTISAN_VERSION
	try {
		$env:ARTISAN_INSTALL_TEST_MODE = "1"
		$env:ARTISAN_INSTALL_TEST_RESOLVE_ONLY = "1"
		$env:ARTISAN_INSTALL_TEST_ARCH = $Architecture
		$env:ARTISAN_VERSION = $Version
		return (& $script_path | ConvertFrom-Json)
	} finally {
		$env:ARTISAN_INSTALL_TEST_MODE = $previous_mode
		$env:ARTISAN_INSTALL_TEST_RESOLVE_ONLY = $previous_resolve
		$env:ARTISAN_INSTALL_TEST_ARCH = $previous_architecture
		$env:ARTISAN_VERSION = $previous_version
	}
}

$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
	$script_path,
	[ref] $null,
	[ref] $errors
) | Out-Null
if ($errors.Count -ne 0) {
	throw "windows.ps1 has PowerShell parse errors: $($errors.Message -join '; ')"
}

$x64 = Resolve-Transport "x64"
if ($x64.target -ne "windows-x64" -or $x64.asset -ne "artisan-bootstrap-windows-x64.exe") {
	throw "Windows x64 selection is incorrect."
}
$arm64 = Resolve-Transport "arm64" "v1.2.3"
if ($arm64.target -ne "windows-arm64") {
	throw "Windows arm64 selection is incorrect."
}
if ($arm64.asset_uri -ne "https://github.com/sandersonstabo/artisan-editor/releases/download/v1.2.3/artisan-bootstrap-windows-arm64.exe") {
	throw "Pinned Windows release URL is incorrect."
}
if ($arm64.checksum_uri -ne "$($arm64.asset_uri).sha256") {
	throw "Windows checksum URL is incorrect."
}
if ($arm64.manifest_uri -ne "https://github.com/sandersonstabo/artisan-editor/releases/download/v1.2.3/release-manifest.json") {
	throw "Pinned Windows manifest URL is incorrect."
}

$source = Get-Content -Raw $script_path
foreach ($required in @(
	"Get-FileHash",
	"[guid]::NewGuid",
	"[System.IO.Directory]::Delete",
	"ValueFromRemainingArguments"
)) {
	if (-not $source.Contains($required)) {
		throw "windows.ps1 is missing required safety behavior: $required"
	}
}

Write-Output "Windows install transport tests passed."
