$ErrorActionPreference = "Stop"

# Windows treats environment keys case-insensitively, while some launchers can
# still pass both `Path` and `PATH`. Start-Process rejects that duplicate block.
$process_path = $env:Path
[Environment]::SetEnvironmentVariable("PATH", $null, [EnvironmentVariableTarget]::Process)
[Environment]::SetEnvironmentVariable("Path", $process_path, [EnvironmentVariableTarget]::Process)

$repository_root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "../../..")).Path
$artifact_release_root = Join-Path $repository_root ".dist/electron-release/win-unpacked"
$artifact_resources = Join-Path $artifact_release_root "resources"
$artifact_asar = Join-Path $artifact_resources "app.asar"
$artifact_executable = Join-Path $artifact_release_root "Artisan Editor.exe"

foreach ($path in @(
	$artifact_asar,
	$artifact_executable
)) {
	if (-not (Test-Path -LiteralPath $path)) {
		throw "Missing packaged desktop evidence path: $path"
	}
}

$asar = Join-Path $repository_root "node_modules/.bin/asar.CMD"
$asar_entries = @(& $asar list $artifact_asar | ForEach-Object { $_.Replace("\", "/") })
if ($LASTEXITCODE -ne 0) {
	throw "Could not enumerate the packaged ASAR"
}

# The editor is the installed renderer: the launcher entry plus the bundled
# static frontend must both be present in the archive.
foreach ($entry in @(
	"/main.js",
	"/frontend/index.html",
	"/frontend/_app"
)) {
	if (
		$asar_entries -notcontains $entry -and
		-not ($asar_entries | Where-Object { $_ -like "$entry/*" })
	) {
		throw "Missing ASAR entry: $entry"
	}
}

# The renderer talks to Forge over plain HTTP/WS; no privileged bridge exists.
foreach ($entry in @(
	"/preload.cjs",
	"/preload.js",
	"/utility.js"
)) {
	if ($asar_entries -contains $entry) {
		throw "A privileged renderer bridge is packaged: $entry"
	}
}

# Forge-owned payload must never ride inside the editor archive.
foreach ($entry in @(
	"/native-runtime",
	"/migrations",
	"/modules/backend/drizzle"
)) {
	if (
		$asar_entries -contains $entry -or
		($asar_entries | Where-Object { $_ -like "$entry/*" })
	) {
		throw "Forge-owned payload leaked into the Electron ASAR: $entry"
	}
}

# The bundled shell must carry the loopback CSP variant so the app-scheme
# renderer can reach the home's Forge, while remaining otherwise strict.
$extract_root = Join-Path ([System.IO.Path]::GetTempPath()) ("artisan-desktop-shell-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $extract_root | Out-Null
try {
	# asar extract-file writes into the working directory.
	Push-Location $extract_root
	try {
		& $asar extract-file $artifact_asar "frontend/index.html"
		if ($LASTEXITCODE -ne 0) {
			throw "Could not extract the packaged renderer shell"
		}
	} finally {
		Pop-Location
	}
	$shell_document = Get-Content -LiteralPath (Join-Path $extract_root "index.html") -Raw
	if ($shell_document -notmatch [regex]::Escape("connect-src 'self' http://127.0.0.1:* ws://127.0.0.1:*")) {
		throw "The packaged renderer shell lacks the loopback Forge CSP allowance"
	}
	if ($shell_document -notmatch [regex]::Escape("object-src 'none'")) {
		throw "The packaged renderer shell weakened its object-src policy"
	}
	if ($shell_document -match "connect-src[^;]*https?://(?!127\.0\.0\.1|\[::1\])") {
		throw "The packaged renderer shell allows a non-loopback connect origin"
	}
} finally {
	Remove-Item -LiteralPath $extract_root -Recurse -Force -ErrorAction SilentlyContinue
}

# The managed editor payload never embeds a parallel Forge lifecycle; `ae`
# owns the daemon and pairing.
$embedded_forge = Join-Path $artifact_resources "artisan-forge"
if (Test-Path -LiteralPath $embedded_forge) {
	throw "The managed Editor payload must not embed a parallel Forge lifecycle"
}

$parallel_installers = @(
	Get-ChildItem -LiteralPath (Split-Path $artifact_release_root -Parent) -File |
		Where-Object {
			$_.Name -like "*Setup*.exe" -or
			$_.Extension -eq ".blockmap"
		}
)
if ($parallel_installers.Count -ne 0) {
	throw "The desktop package emitted a parallel installer lifecycle"
}

Write-Output (
	"Packaged desktop renderer evidence: " +
	(@{
		asar_entries = $asar_entries.Count
		bundled_frontend = $true
		forge_payload_embedded = $false
		loopback_csp = $true
		managed_distribution_payload = $true
		ok = $true
		privileged_bridge = $false
	} | ConvertTo-Json -Compress)
)
