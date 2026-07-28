$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$development = Join-Path $workspace ".dist\dev"
$profile_home = Join-Path $development "forge-home"

New-Item -ItemType Directory -Force -Path $profile_home | Out-Null

$env:ARTISAN_HOME = $profile_home

& cargo run -p artisan-editor-cli -- @args
exit $LASTEXITCODE
