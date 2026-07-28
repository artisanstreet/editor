$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$development = Join-Path $workspace ".dist\dev"
$install_root = Join-Path $development "install-root"

New-Item -ItemType Directory -Force -Path $install_root | Out-Null

$env:ARTISAN_INSTALL_ROOT = $install_root
$env:ARTISAN_HOME = $install_root

# pnpm forwards the literal `--` separator into the script's arguments, so
# both calling forms reach the bootstrap cleanly; only the leading separator
# is dropped.
$forwarded = if ($args.Count -gt 0 -and $args[0] -eq "--") { @($args | Select-Object -Skip 1) } else { @($args) }

& cargo run -p artisan-bootstrap -- @forwarded
exit $LASTEXITCODE
