[CmdletBinding()]
param([string]$Root = (Split-Path -Parent $PSScriptRoot))
python "$PSScriptRoot/verify_gpui_pin.py" --root $Root
exit $LASTEXITCODE
