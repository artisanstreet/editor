$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$development = Join-Path $workspace ".dist\dev"
$forge_home = Join-Path $development "forge-home"

New-Item -ItemType Directory -Force -Path $forge_home | Out-Null

$env:ARTISAN_HOME = $forge_home

# pnpm forwards the literal `--` separator into the script's arguments, so
# both `pnpm run dev:ae open` and `pnpm run dev:ae -- open` reach the CLI
# cleanly; only the leading separator is dropped. The outer @(...) must wrap
# the whole `if`: Windows PowerShell collapses a one-element statement result
# to a scalar string, and splatting a scalar over a native command splits it
# into characters.
$forwarded = @(if ($args.Count -gt 0 -and $args[0] -eq "--") { $args | Select-Object -Skip 1 } else { $args })

& cargo run -p artisan-editor-cli -- @forwarded
exit $LASTEXITCODE
