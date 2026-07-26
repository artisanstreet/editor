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
$artifact_forge_root = Join-Path $artifact_resources "artisan-forge"
$artifact_forge_executable = Join-Path $artifact_forge_root "Artisan Forge.exe"
$artifact_forge_entry = Join-Path $artifact_forge_root "host.js"
$artifact_forge_native = Join-Path $artifact_forge_root "native-runtime"
$artifact_forge_node = Join-Path $artifact_forge_root "node.exe"
$artifact_ae_command = Join-Path $artifact_forge_root "ae.cmd"
$artifact_ae_entry = Join-Path $artifact_forge_root "ae.js"
$artifact_path_installer = Join-Path $artifact_forge_root "update-user-path.ps1"

foreach ($path in @(
	$artifact_asar,
	$artifact_executable,
	$artifact_forge_executable,
	$artifact_forge_entry,
	$artifact_forge_native,
	$artifact_forge_node,
	$artifact_ae_command,
	$artifact_ae_entry,
	$artifact_path_installer
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
	"/main.js",
	"/preload.cjs"
)) {
	if (
		$asar_entries -notcontains $entry -and
		-not ($asar_entries | Where-Object { $_ -like "$entry/*" })
	) {
		throw "Missing ASAR entry: $entry"
	}
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

$smoke_root = Join-Path (
	[System.IO.Path]::GetTempPath()
) ("artisan-packaged-daemon-smoke-" + [guid]::NewGuid().ToString("N"))
$isolated_release_root = Join-Path $smoke_root "application"
$stdout_path = Join-Path $smoke_root "stdout.log"
$stderr_path = Join-Path $smoke_root "stderr.log"
New-Item -ItemType Directory -Path $smoke_root | Out-Null
Copy-Item -LiteralPath $artifact_release_root -Destination $isolated_release_root -Recurse

$executable = Join-Path $isolated_release_root "Artisan Editor.exe"
$previous_smoke = $env:ARTISAN_PACKAGED_SMOKE
$previous_user_data = $env:ARTISAN_PACKAGED_SMOKE_USER_DATA
$previous_node_path = $env:NODE_PATH
$process = $null

try {
	$env:ARTISAN_PACKAGED_SMOKE = "1"
	$env:ARTISAN_PACKAGED_SMOKE_USER_DATA = Join-Path $smoke_root "user-data"
	$env:NODE_PATH = ""
	$process = Start-Process `
		-FilePath $executable `
		-WorkingDirectory $isolated_release_root `
		-PassThru `
		-RedirectStandardOutput $stdout_path `
		-RedirectStandardError $stderr_path `
		-WindowStyle Hidden

	$deadline = [DateTime]::UtcNow.AddSeconds(60)
	while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
		Start-Sleep -Milliseconds 100
		$process.Refresh()
	}
	if (-not $process.HasExited) {
		Stop-Process -Id $process.Id -Force
		throw "Packaged desktop daemon smoke exceeded its 60-second deadline"
	}
	$process.WaitForExit()

	$records = @(
		Get-Content -LiteralPath $stdout_path |
			ForEach-Object {
				try {
					$_ | ConvertFrom-Json
				} catch {
					$null
				}
			} |
			Where-Object { $_ -and $_.kind -eq "artisan:packaged-smoke" }
	)
	if ($records.Count -ne 1 -or -not $records[0].ok) {
		throw "Packaged smoke did not emit one successful record: stdout=$(Get-Content -Raw -LiteralPath $stdout_path) stderr=$(Get-Content -Raw -LiteralPath $stderr_path)"
	}

	$record = $records[0]
	if (
		-not $record.forge_pid -or
		$record.forge_pid -eq $process.Id -or
		-not $record.forge_websocket_endpoint.StartsWith("ws://127.0.0.1:") -or
		-not $record.renderer.has_native_bridge -or
		$record.renderer.title -ne "Artisan Editor" -or
		-not $record.renderer.body.Contains("No threads yet. Create one from the sidebar.")
	) {
		throw "Packaged smoke did not prove a separate Forge process and connected renderer: $($record | ConvertTo-Json -Compress -Depth 6)"
	}

	Start-Sleep -Milliseconds 250
	if (Get-Process -Id $record.forge_pid -ErrorAction SilentlyContinue) {
		throw "Artisan Forge survived desktop smoke shutdown: PID $($record.forge_pid)"
	}

	Write-Output (
		"Packaged daemon smoke evidence: " +
		($record | ConvertTo-Json -Compress -Depth 6)
	)
} finally {
	if ($process -and -not $process.HasExited) {
		Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
	}
	$env:ARTISAN_PACKAGED_SMOKE = $previous_smoke
	$env:ARTISAN_PACKAGED_SMOKE_USER_DATA = $previous_user_data
	$env:NODE_PATH = $previous_node_path
	for ($attempt = 1; $attempt -le 40 -and (Test-Path -LiteralPath $smoke_root); $attempt += 1) {
		try {
			Remove-Item -LiteralPath $smoke_root -Force -Recurse -ErrorAction Stop
		} catch {
			if ($attempt -eq 40) {
				throw
			}
			Start-Sleep -Milliseconds 250
		}
	}
}
