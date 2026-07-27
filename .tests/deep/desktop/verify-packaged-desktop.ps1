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
foreach ($entry in @(
	"/main.js"
)) {
	if (
		$asar_entries -notcontains $entry -and
		-not ($asar_entries | Where-Object { $_ -like "$entry/*" })
	) {
		throw "Missing ASAR entry: $entry"
	}
}
if ($asar_entries -contains "/preload.cjs") {
	throw "The dormant Electron preload bridge is still packaged"
}
if ($asar_entries -contains "/utility.js") {
	throw "The legacy Electron utility backend is still packaged"
}
foreach ($entry in @(
	"/native-runtime",
	"/.dist/frontend",
	"/modules/backend/drizzle"
)) {
	if (
		$asar_entries -contains $entry -or
		($asar_entries | Where-Object { $_ -like "$entry/*" })
	) {
		throw "Forge-owned payload leaked into the thin Electron ASAR: $entry"
	}
}

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
	"Packaged stateless desktop evidence: " +
	(@{
		asar_entries = $asar_entries.Count
		forge_payload_embedded = $false
		managed_distribution_payload = $true
		ok = $true
	} | ConvertTo-Json -Compress)
)
