param(
	# Optional loopback origin the pairing fragment should land on. `pnpm run
	# dev:pair` points this at the Vite dev server, whose proxy forwards the
	# exchange to the Forge; without it the built frontend on the Forge's own
	# origin is opened.
	[string]$Origin
)

$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$runtime = Join-Path $workspace ".dist\forge"
$development = Join-Path $workspace ".dist\dev"

$env:ARTISAN_HOME = Join-Path $development "forge-home"

if ($Origin) {
	& (Join-Path $runtime "ae.cmd") open --profile browser-dev --origin $Origin
} else {
	& (Join-Path $runtime "ae.cmd") open --profile browser-dev
}
if ($LASTEXITCODE -ne 0) {
	throw "Could not open a paired browser for the development Forge; is `pnpm run dev:forge` running?"
}
