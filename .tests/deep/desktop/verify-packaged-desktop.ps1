$ErrorActionPreference = "Stop"

$release_root = Join-Path $PSScriptRoot "../../../.dist/electron-release/win-unpacked"
$resources = Join-Path $release_root "resources"
$asar = Join-Path $resources "app.asar"
$unpacked = Join-Path $resources "app.asar.unpacked/.dist/desktop/native-runtime"
$executable = Join-Path $release_root "Artisan Editor.exe"

function Get-PackagedSmokeProcesses {
	@(Get-CimInstance Win32_Process -Filter "Name = 'Artisan Editor.exe'" |
		Where-Object { $_.ExecutablePath -eq $executable })
}

function Stop-PackagedSmokeProcesses {
	param([string]$Reason)
	$stale = @(Get-PackagedSmokeProcesses)
	if ($stale.Count -eq 0) { return }
	foreach ($stale_process in $stale) {
		& taskkill.exe /PID $stale_process.ProcessId /T /F | Out-Null
	}
	Start-Sleep -Milliseconds 250
	$remaining = @(Get-PackagedSmokeProcesses)
	if ($remaining.Count -ne 0) {
		throw "Could not clean exact packaged smoke processes after ${Reason}: $($remaining.ProcessId -join ', ')"
	}
}

foreach ($path in @($asar, $unpacked, $executable)) {
	if (-not (Test-Path -LiteralPath $path)) { throw "Missing packaged desktop evidence path: $path" }
}

# A prior aborted smoke must not satisfy the single-instance lock or survive this gate.
Stop-PackagedSmokeProcesses -Reason "preflight"

$asar_entries = @(pnpm exec asar list $asar | ForEach-Object { $_.Replace("\", "/") })
if ($LASTEXITCODE -ne 0) { throw "Could not enumerate the packaged ASAR" }
foreach ($entry in @("/.dist/desktop/main.js", "/.dist/desktop/preload.cjs", "/.dist/desktop/utility.js", "/.dist/frontend/index.html", "/modules/backend/drizzle")) {
	if ($asar_entries -notcontains $entry -and -not ($asar_entries | Where-Object { $_ -like "$entry/*" })) { throw "Missing ASAR entry: $entry" }
}
foreach ($native in @("node-pty", "@artisan/bounded-file-store-native/bounded_file_store_native.win32-x64-msvc.node", "@koromix/koffi-win32-x64/win32_x64/koffi.node")) {
	if (-not (Test-Path -LiteralPath (Join-Path $unpacked $native))) { throw "Missing unpacked native runtime: $native" }
}

$smoke_root = Join-Path ([System.IO.Path]::GetTempPath()) ("artisan-packaged-smoke-" + [guid]::NewGuid().ToString("N"))
$stdout_path = Join-Path $smoke_root "stdout.log"
$stderr_path = Join-Path $smoke_root "stderr.log"
New-Item -ItemType Directory -Path $smoke_root | Out-Null
$previous_smoke = $env:ARTISAN_PACKAGED_SMOKE
$previous_user_data = $env:ARTISAN_PACKAGED_SMOKE_USER_DATA
$previous_smoke_root = $env:ARTISAN_PACKAGED_SMOKE_ROOT
$previous_node_path = $env:NODE_PATH
$process = $null
try {
	$env:ARTISAN_PACKAGED_SMOKE = "1"
	$env:ARTISAN_PACKAGED_SMOKE_USER_DATA = (Join-Path $smoke_root "user-data")
	$env:ARTISAN_PACKAGED_SMOKE_ROOT = $smoke_root
	# The smoke must resolve only the staged runtime, not a caller-provided module lookup path.
	$env:NODE_PATH = ""
	$process = Start-Process -FilePath $executable -PassThru -RedirectStandardOutput $stdout_path -RedirectStandardError $stderr_path
	$deadline = [DateTime]::UtcNow.AddSeconds(60)
	while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
		Start-Sleep -Milliseconds 200
		$process.Refresh()
	}
	if (-not $process.HasExited) {
		& taskkill.exe /PID $process.Id /T /F | Out-Null
		throw "Packaged desktop smoke exceeded its 60-second deadline"
	}
	$process.WaitForExit()
	# Electron's bootstrap Process can make ExitCode unavailable after it hands the
	# application lifecycle to the browser process. The signed JSON evidence below
	# remains mandatory; reject a concrete non-zero code when Windows provides one.
	$exit_code = $process.ExitCode
	if ($null -ne $exit_code -and $exit_code -ne 0) {
		throw "Packaged desktop smoke failed with exit code ${exit_code}: stdout=$(Get-Content -Raw -LiteralPath $stdout_path) stderr=$(Get-Content -Raw -LiteralPath $stderr_path)"
	}
	$records = @(Get-Content -LiteralPath $stdout_path | ForEach-Object {
		try { $_ | ConvertFrom-Json } catch { $null }
	} | Where-Object { $_ -and $_.kind -eq "artisan:packaged-smoke" })
	if ($records.Count -ne 1 -or -not $records[0].ok) { throw "Packaged desktop smoke did not emit one successful evidence record" }
	$record = $records[0]
	if (-not $record.native_load.initial.bounded_native_binding_path -or -not $record.native_load.initial.koffi_native_binding_path -or -not $record.native_load.initial.node_pty_module_path -or -not $record.native_load.restarted.bounded_native_binding_path -or -not $record.native_load.restarted.koffi_native_binding_path -or -not $record.native_load.restarted.node_pty_module_path) { throw "Packaged desktop smoke did not prove native bindings were opened in both utility epochs" }
	if ($record.restart.previous_utility_epoch -ge $record.restart.next_utility_epoch -or -not $record.restart.previous_utility_exit_observed -or -not $record.restart.kill_accepted -or -not $record.native_load.initial.utility_pid -or -not $record.native_load.restarted.utility_pid -or $record.native_load.initial.utility_pid -eq $record.native_load.restarted.utility_pid) { throw "Packaged desktop smoke did not prove utility replacement" }
	if ($record.native_load.restarted.utility_epoch -ne $record.restart.next_utility_epoch -or -not $record.forward_generation) { throw "Packaged desktop smoke did not prove restarted native readiness and forward progress" }
	$expected_asar_root = (Resolve-Path -LiteralPath $asar).Path
	$expected_unpacked_application_root = (Resolve-Path -LiteralPath (Join-Path $resources "app.asar.unpacked")).Path
	$expected_native_root = (Resolve-Path -LiteralPath $unpacked).Path
	foreach ($path in @($record.native_load.initial.bounded_native_binding_path, $record.native_load.initial.koffi_native_binding_path, $record.native_load.initial.node_pty_module_path, $record.native_load.restarted.bounded_native_binding_path, $record.native_load.restarted.koffi_native_binding_path, $record.native_load.restarted.node_pty_module_path)) {
		$physical_path = $path
		if ($physical_path.StartsWith($expected_asar_root + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
			$relative_path = $physical_path.Substring($expected_asar_root.Length).TrimStart([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
			$physical_path = Join-Path $expected_unpacked_application_root $relative_path
		}
		$resolved_path = (Resolve-Path -LiteralPath $physical_path).Path
		if (-not $resolved_path.StartsWith($expected_native_root, [System.StringComparison]::OrdinalIgnoreCase)) { throw "Packaged desktop smoke resolved a native module outside app.asar.unpacked: $resolved_path" }
	}
	$expected_smoke_root = [System.IO.Path]::GetFullPath($smoke_root)
	foreach ($store_root in @($record.native_load.initial.native_store_root, $record.native_load.restarted.native_store_root)) {
		if (-not $store_root -or -not ([System.IO.Path]::GetFullPath($store_root)).StartsWith($expected_smoke_root, [System.StringComparison]::OrdinalIgnoreCase)) { throw "Packaged desktop smoke opened its native store outside the unique smoke root" }
	}
	if (-not $record.mounted_ui -or -not $record.mounted_ui.wide.bridge_available -or -not $record.mounted_ui.wide.accessible_names.Contains("New chat") -or -not $record.mounted_ui.wide.accessible_names.Contains("Open Marketplace")) { throw "Packaged desktop smoke did not prove mounted accessible renderer controls" }
	foreach ($keyboard_activation in @($record.mounted_ui.keyboard_thread_counts.initial, $record.mounted_ui.keyboard_thread_counts.restarted)) {
		if ($keyboard_activation.focused_name -ne "New chat" -or $keyboard_activation.thread_count -lt 1 -or -not $keyboard_activation.event.is_trusted -or $keyboard_activation.event.detail -ne 0 -or -not $keyboard_activation.event.active_before_click) { throw "Packaged desktop smoke did not prove a trusted Electron keyboard activation on the focused New chat control" }
	}
	if ($record.mounted_ui.keyboard_thread_counts.restarted.thread_count -le $record.mounted_ui.keyboard_thread_counts.initial.thread_count) { throw "Packaged desktop smoke did not prove renderer keyboard commands across utility restart" }
	$interactions = $record.mounted_ui.product_interactions
	if (-not $interactions -or -not $interactions.activity_bridge -or -not $interactions.marketplace.open -or -not $interactions.marketplace.focus_restored -or -not $interactions.marketplace.click.is_trusted -or $interactions.marketplace.click.detail -ne 1 -or -not $interactions.composer.is_trusted -or -not $interactions.composer.value.Contains("h") -or -not $interactions.editor_restore.restored -or -not $interactions.editor_restore.click.is_trusted -or -not $interactions.unavailable.no_active_file -or -not $interactions.unavailable.no_terminal_sessions) { throw "Packaged desktop smoke did not prove mounted activity, Marketplace, mode, composer, and unavailable-state interactions" }
	if (@($interactions.modes).Count -ne 4 -or (@($interactions.modes | Where-Object { -not $_.click.is_trusted -or $_.click.detail -ne 1 -or $_.visible_region -notin @("Chat", "Editor", "Orchestrator") }).Count -ne 0)) { throw "Packaged desktop smoke did not prove trusted mounted workspace mode interactions" }
	if (-not $interactions.right_tab_keyboard) { throw "Packaged desktop smoke did not prove right-pane tab keyboard reachability" }
	if ($record.mounted_ui.wide.grid_template_columns.Split(" ").Count -lt 3 -or $record.mounted_ui.narrow.grid_template_columns.Split(" ").Count -ne 1 -or -not $record.mounted_ui.wide.left_visible -or -not $record.mounted_ui.wide.right_visible -or $record.mounted_ui.narrow.left_visible -or $record.mounted_ui.narrow.right_visible -or $record.mounted_ui.zoom_factor -ne 2) { throw "Packaged desktop smoke did not prove wide/narrow responsive layout and 200% zoom" }
	Write-Output ("Packaged smoke evidence: " + ($record | ConvertTo-Json -Compress -Depth 10))
} finally {
	if ($process -and -not $process.HasExited) { & taskkill.exe /PID $process.Id /T /F | Out-Null }
	# Electron's bootstrap PID can exit after handing ownership to its process tree, so
	# also remove only remaining processes at this exact release-artifact executable.
	Stop-PackagedSmokeProcesses -Reason "cleanup"
	$env:ARTISAN_PACKAGED_SMOKE = $previous_smoke
	$env:ARTISAN_PACKAGED_SMOKE_USER_DATA = $previous_user_data
	$env:ARTISAN_PACKAGED_SMOKE_ROOT = $previous_smoke_root
	$env:NODE_PATH = $previous_node_path
	# Chromium can release its LevelDB handles a few seconds after its executable
	# tree disappears. Keep cleanup mandatory, but allow that bounded close delay.
	for ($attempt = 1; $attempt -le 40 -and (Test-Path -LiteralPath $smoke_root); $attempt += 1) {
		try {
			Remove-Item -LiteralPath $smoke_root -Force -Recurse -ErrorAction Stop
		} catch {
			if ($attempt -eq 40) { throw }
			Start-Sleep -Milliseconds 250
		}
	}
}
