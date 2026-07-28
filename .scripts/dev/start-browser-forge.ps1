$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$runtime = Join-Path $workspace ".dist\forge"
$development = Join-Path $workspace ".dist\dev"
$forge_home = Join-Path $development "forge-home"

New-Item -ItemType Directory -Force -Path $development | Out-Null

$env:ARTISAN_HOME = $forge_home

# Static web hosting is a development capability: the dev home is the only
# composition that opts in. Installed homes leave it off and render through
# the Electron editor instead.
& (Join-Path $runtime "ae.cmd") `
	setup `
	--mode local `
	--serve-frontend `
	--data-root (Join-Path $development "browser-forge") `
	--listen-port 4848
if ($LASTEXITCODE -ne 0) {
	throw "Could not configure the browser development Forge"
}

& (Join-Path $runtime "ae.cmd") start --foreground
