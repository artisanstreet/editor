$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($env:ARTISAN_RUN_NATIVE_ADDON_SMOKE -ne "1") {
	throw "Set ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 only in an approved native verification environment"
}

$build_local_gnu = Join-Path $PSScriptRoot "build-local-gnu.ps1"
$saved_environment = @{
	ARTISAN_RUN_NATIVE_CRASH_SMOKE = $env:ARTISAN_RUN_NATIVE_CRASH_SMOKE
	ARTISAN_RUN_NATIVE_RACE_SMOKE = $env:ARTISAN_RUN_NATIVE_RACE_SMOKE
	ARTISAN_RUN_NATIVE_REPLACE_SMOKE = $env:ARTISAN_RUN_NATIVE_REPLACE_SMOKE
	UV_THREADPOOL_SIZE = $env:UV_THREADPOOL_SIZE
}

function restore_environment_variable {
	param(
		[string]$name,
		[AllowNull()][string]$value
	)

	if ($null -eq $value) {
		Remove-Item "Env:$name" -ErrorAction SilentlyContinue

		return
	}

	Set-Item "Env:$name" $value
}

try {
	$env:ARTISAN_RUN_NATIVE_REPLACE_SMOKE = "1"
	Remove-Item Env:ARTISAN_RUN_NATIVE_RACE_SMOKE -ErrorAction SilentlyContinue
	Remove-Item Env:ARTISAN_RUN_NATIVE_CRASH_SMOKE -ErrorAction SilentlyContinue
	& $build_local_gnu

	$configured_threadpool_size = 0
	[void][int]::TryParse($env:UV_THREADPOOL_SIZE, [ref]$configured_threadpool_size)

	$env:ARTISAN_RUN_NATIVE_RACE_SMOKE = "1"
	$env:ARTISAN_RUN_NATIVE_CRASH_SMOKE = "1"
	$env:UV_THREADPOOL_SIZE = [Math]::Max(4, $configured_threadpool_size)
	& $build_local_gnu -TestHooks
} finally {
	foreach ($entry in $saved_environment.GetEnumerator()) {
		restore_environment_variable -name $entry.Key -value $entry.Value
	}
}
