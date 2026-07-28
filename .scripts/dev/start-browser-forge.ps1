$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$runtime = Join-Path $workspace ".dist\forge"
$development = Join-Path $workspace ".dist\dev"
$profile_home = Join-Path $development "forge-home"

New-Item -ItemType Directory -Force -Path $development | Out-Null

$env:ARTISAN_HOME = $profile_home

# Static web hosting is a development capability: the browser-dev profile is
# the only composition that opts in. Installed profiles leave it off and render
# through the Electron editor instead.
& (Join-Path $runtime "ae.cmd") `
	setup `
	--profile browser-dev `
	--mode local `
	--serve-frontend `
	--data-root (Join-Path $development "browser-forge") `
	--listen-port 4848
if ($LASTEXITCODE -ne 0) {
	throw "Could not configure the browser development Forge profile"
}

& (Join-Path $runtime "ae.cmd") start --profile browser-dev --foreground
