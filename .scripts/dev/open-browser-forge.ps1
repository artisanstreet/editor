$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$runtime = Join-Path $workspace ".dist\forge"
$development = Join-Path $workspace ".dist\dev"

$env:ARTISAN_HOME = Join-Path $development "forge-home"

& (Join-Path $runtime "ae.cmd") open --profile browser-dev
if ($LASTEXITCODE -ne 0) {
	throw "Could not open a paired browser for the development Forge; is `pnpm run dev:forge` running?"
}
