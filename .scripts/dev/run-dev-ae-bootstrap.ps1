$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$development = Join-Path $workspace ".dist\dev"
$install_root = Join-Path $development "install-root"

New-Item -ItemType Directory -Force -Path $install_root | Out-Null

$env:ARTISAN_INSTALL_ROOT = $install_root
$env:ARTISAN_HOME = $install_root

& cargo run -p artisan-bootstrap -- @args
exit $LASTEXITCODE
