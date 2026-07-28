$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$runtime = Join-Path $workspace ".dist\forge"
$development = Join-Path $workspace ".dist\dev"

$env:ARTISAN_HOME = Join-Path $development "forge-home"

$forge_origin = if ($env:ARTISAN_FORGE_DEV_ORIGIN) { $env:ARTISAN_FORGE_DEV_ORIGIN } else { "http://127.0.0.1:4848" }
$frontend_port = if ($env:ARTISAN_FRONTEND_DEV_PORT) { [int]$env:ARTISAN_FRONTEND_DEV_PORT } else { 4849 }
$frontend_origin = "http://127.0.0.1:$frontend_port"

$forge_ready = $false
try {
	$health = Invoke-WebRequest -Uri "$forge_origin/health" -UseBasicParsing -TimeoutSec 2
	$forge_ready = $health.StatusCode -eq 200
} catch {}

Write-Host "Artisan frontend dev server (HMR): $frontend_origin"
Write-Host "Forge control/stream traffic proxies to: $forge_origin"
if ($forge_ready) {
	Write-Host "A paired browser opens automatically once Vite is up. If it does not, run: pnpm run dev:pair"
} else {
	Write-Host "The development Forge is not answering on $forge_origin."
	Write-Host "Start it with 'pnpm run dev:forge', then pair a browser with 'pnpm run dev:pair'."
}

# Deliver the one-time pairing fragment onto the dev-server origin as soon as
# Vite answers. `ae open --origin` validates the loopback origin and the
# fragment capability never touches disk; the exchange itself flows through the
# Vite proxy back to the Forge, so the whole session stays same-origin.
$pair_job = $null
if ($forge_ready) {
	$ae = Join-Path $runtime "ae.cmd"
	$pair_job = Start-Job -ScriptBlock {
		param($origin, $ae_command, $artisan_home)
		$env:ARTISAN_HOME = $artisan_home
		for ($attempt = 0; $attempt -lt 120; $attempt += 1) {
			try {
				$response = Invoke-WebRequest -Uri $origin -UseBasicParsing -TimeoutSec 2
				if ($response.StatusCode -eq 200) {
					& $ae_command open --origin $origin
					return
				}
			} catch {}
			Start-Sleep -Milliseconds 500
		}
	} -ArgumentList $frontend_origin, $ae, $env:ARTISAN_HOME
}

try {
	pnpm --filter @artisan/frontend run dev
} finally {
	if ($pair_job) {
		try { Stop-Job $pair_job -ErrorAction Stop } catch {}
		try { Remove-Job $pair_job -Force -ErrorAction Stop } catch {}
	}
}
