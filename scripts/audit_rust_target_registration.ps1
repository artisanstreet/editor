[CmdletBinding()]
param([string]$Root = (Split-Path -Parent (Split-Path -Parent $PSCommandPath)))
& python (Join-Path $Root 'scripts/audit_rust_target_registration.py')
exit $LASTEXITCODE
